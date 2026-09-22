import { EnvironmentMonitor, observerUrl } from "./browser-environment.mjs";
import { spawn } from "node:child_process";
import { Buffer } from "node:buffer";
import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import { promises as fsp } from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { URL } from "node:url";
import { chromium, expect } from "@playwright/test";
import { strToU8, zipSync } from "fflate";
import {
  ARTIFACT_SCAN_BUFFER_BYTES,
  ArtifactSafetyError,
  BytePatternMatcher,
  DEFAULT_ARTIFACT_SAFETY_LIMITS,
  StreamingPatternScanner,
  unzipArchiveBounded,
} from "./browser-artifact-safety.mjs";
import { diagnoseFrontend } from "./browser-frontend-health.mjs";
import {
  ConcurrencyError,
  WORKER_MEMORY_BUDGET_BYTES,
  cancellable,
  declaredParallelCapacity,
  describeConcurrencyGroups,
  normalizeConcurrencyPolicy,
  normalizeSessionRole,
  normalizeTestConcurrency,
  resolveWorkerLimit,
  runBounded,
} from "./browser-parallel.mjs";

const SCHEMA_VERSION = "5.0.0";
const RUNNER_VERSION = "1.0.0";
const MINIMUM_NODE_VERSION = "22.22.2";
const MAX_STDIN_BYTES = 4 * 1024 * 1024;
const MAX_SUITE_TESTS = 100;
const MAX_STEPS_PER_TEST = 500;
const MAX_EVENT_ENTRIES = 500;
const MAX_TEXT_LENGTH = 8_192;
const WIDGET_CSS_DIAGNOSTIC = {
  code: "mendix_widget_css_missing",
  message:
    "The Mendix aggregate widget stylesheet returned 404. Check MPK CSS and generated CSS imports; compare a clean build of a copy on a Windows local drive when using a UNC share. Do not create empty CSS or ignore this failure.",
  documentation:
    "https://github.com/GG-O-BP/mendimaru/blob/main/docs/widget-css-diagnostics.md",
};
const PRIVATE_STYLE = `
  input[type="password"],
  [data-mendimaru-private="true"],
  .mx-name-MendimaruPrivate {
    opacity: 0 !important;
    color: transparent !important;
    caret-color: transparent !important;
    text-shadow: none !important;
    -webkit-text-security: disc !important;
  }
`;

const require = createRequire(import.meta.url);
const playwrightVersion = readPackageVersion("@playwright/test/package.json");

class RunnerError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "RunnerError";
    this.code = code;
  }
}

const command = process.argv[2];
try {
  let result;
  if (command === "doctor") {
    result = await doctor();
  } else if (command === "install") {
    await installChromium();
    result = await doctor();
  } else if (command === "frontend-health") {
    requireSupportedNode();
    result = await diagnoseFrontend(
      chromium,
      JSON.parse(process.env.MENDIMARU_FRONTEND_REQUEST_JSON),
    );
  } else if (command === "run") {
    result = await run(await readRequest());
  } else {
    throw new RunnerError("invalid_request", "unknown browser runner command");
  }
  process.stdout.write(`${JSON.stringify({ ok: true, data: result })}\n`);
} catch (error) {
  const code = error instanceof RunnerError ? error.code : "runner_failed";
  process.stdout.write(
    `${JSON.stringify({
      ok: false,
      error: { code, message: safeRunnerMessage(code) },
    })}\n`,
  );
  process.exitCode = 1;
}

function readPackageVersion(specifier) {
  const filename = require.resolve(specifier);
  const value = JSON.parse(fs.readFileSync(filename, "utf8"));
  return String(value.version);
}

async function doctor() {
  const nodeSupported = versionAtLeast(
    process.versions.node,
    MINIMUM_NODE_VERSION,
  );
  const executablePath = chromium.executablePath();
  const installed = await directFileExists(executablePath);
  let launchable = false;
  let browserVersion = null;
  if (installed) {
    let browser;
    try {
      browser = await chromium.launch({ headless: true });
      browserVersion = browser.version();
      launchable = true;
    } catch {
      launchable = false;
    } finally {
      await browser?.close().catch(() => {});
    }
  }
  return {
    schemaVersion: SCHEMA_VERSION,
    runnerVersion: RUNNER_VERSION,
    ready: nodeSupported && installed && launchable,
    nodeVersion: process.versions.node,
    minimumNodeVersion: MINIMUM_NODE_VERSION,
    nodeSupported,
    playwrightVersion,
    chromium: {
      installed,
      launchable,
      ...(browserVersion ? { version: browserVersion } : {}),
    },
    downloadPolicy: "explicit-only",
  };
}

async function installChromium() {
  requireSupportedNode();
  const cli = path.join(
    path.dirname(require.resolve("playwright/package.json")),
    "cli.js",
  );
  const exitCode = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cli, "install", "chromium"], {
      env: process.env,
      stdio: ["ignore", "ignore", "ignore"],
      windowsHide: true,
    });
    child.once("error", reject);
    child.once("exit", (code, signal) =>
      resolve(signal === null && code !== null ? code : 1),
    );
  }).catch(() => 1);
  if (exitCode !== 0) {
    throw new RunnerError(
      "chromium_install_failed",
      "the explicit Chromium installation failed",
    );
  }
}

async function run(rawRequest) {
  requireSupportedNode();
  const request = validateRequest(rawRequest);
  const sessionRole = normalizeSessionRole(request.sessionRole);
  const concurrencyPolicy = mapConcurrencyErrors(() =>
    normalizeConcurrencyPolicy(request.policy.concurrency),
  );
  const suite = validateSuite(request.suite, sessionRole);
  const outputDirectory = await validateOutputDirectory(
    request.outputDirectory,
  );
  const baseUrl = validateBaseUrl(request.baseUrl);
  const assetMirrorUrl = validateOptionalAssetMirrorUrl(request.assetMirrorUrl);
  const { secrets, storageState } = await collectSecrets(suite);
  const plans = suite.tests.map((test) => test.concurrency);
  // CPU and memory headroom, and what the suite could ever overlap, bound the
  // requested lanes. The effective number and its limit are reported.
  const concurrency = resolveWorkerLimit({
    requested: concurrencyPolicy.workers,
    capacity: declaredParallelCapacity(plans),
    testCount: suite.tests.length,
    cpuCount: availableParallelism(),
    availableMemoryBytes: availableMemoryBytes(),
    memoryBudgetBytes: WORKER_MEMORY_BUDGET_BYTES,
  });
  const startedAt = new Date().toISOString();
  const environment = new EnvironmentMonitor(
    request.environmentObserverUrl,
    request.sessionId,
  );
  await environment.start();
  // One Chromium process per lane. Lane 0 is launched eagerly so a broken
  // toolchain still fails exactly like the historical single-worker path; a
  // crash in one lane never removes another lane's browser or artifacts.
  const lanes = createBrowserLanes(concurrency.workers);
  let browserVersion;
  try {
    browserVersion = (await lanes.browser(0)).version();
  } catch {
    await lanes.close();
    await environment.finish();
    throw new RunnerError(
      "chromium_unavailable",
      "the pinned Playwright Chromium build is unavailable",
    );
  }

  const results = new Array(suite.tests.length);
  const perTestFiles = new Array(suite.tests.length);
  const perTestConsole = new Array(suite.tests.length);
  const perTestPageErrors = new Array(suite.tests.length);
  const perTestNetworkFailures = new Array(suite.tests.length);
  const mirrorCounters = { intercepted: 0 };
  let maxObservedParallel;
  try {
    const scheduled = await runBounded({
      plans,
      workers: concurrency.workers,
      testTimeoutMilliseconds: concurrencyPolicy.testTimeoutMilliseconds,
      isInterrupted: () => environment.interrupted,
      // A test that never ran still has to carry a valid verdict. Only an
      // environment change may be reported as `skipped`; every other reason
      // is an invalid verdict, so it is reported as `failed` with the reason
      // preserved rather than silently dropped.
      skip: (index, invalidatedBy) => ({
        value: {
          summary: {
            name: suite.tests[index].name,
            outcome:
              invalidatedBy === "environment-change" ? "skipped" : "failed",
            completedSteps: 0,
            totalSteps:
              suite.beforeEach.length + suite.tests[index].steps.length,
            ...(invalidatedBy === "environment-change"
              ? { invalidatedBy }
              : {
                  failure: `the test did not run (${invalidatedBy})`,
                  invalidatedBy,
                }),
          },
          files: [],
          consoleEntries: [],
          pageErrors: [],
          networkFailures: [],
        },
      }),
      execute: async (index, signal, lane) =>
        runTest({
          environment,
          browser: await lanes.browser(lane),
          baseUrl,
          assetMirrorUrl,
          mirrorCounters,
          index,
          outputDirectory,
          policy: request.policy,
          secrets,
          signal,
          storageState,
          suite,
          test: suite.tests[index],
        }),
    });
    maxObservedParallel = scheduled.maxObservedParallel;
    for (let index = 0; index < suite.tests.length; index += 1) {
      const outcome = scheduled.outcomes[index];
      // A lane that crashed outside a test (for example a browser that never
      // launched) still produces one explicit, invalid result for that test.
      const result = outcome?.value ?? laneFailure(suite, index, outcome);
      results[index] = result.summary;
      perTestFiles[index] = result.files;
      perTestConsole[index] = result.consoleEntries;
      perTestPageErrors[index] = result.pageErrors;
      perTestNetworkFailures[index] = result.networkFailures;
    }
  } finally {
    await lanes.close();
    await environment.finish();
  }

  // Completion order is not the report order: artifacts, diagnostics and test
  // results are always assembled in suite declaration order.
  const files = perTestFiles.flat();
  const allConsole = perTestConsole.flat();
  const allPageErrors = perTestPageErrors.flat();
  const allNetworkFailures = perTestNetworkFailures.flat();

  await writeJsonArtifact(
    outputDirectory,
    "console.json",
    { entries: allConsole.slice(0, MAX_EVENT_ENTRIES) },
    files,
    "diagnostic",
    "application/json",
  );
  await writeJsonArtifact(
    outputDirectory,
    "page-errors.json",
    { entries: allPageErrors.slice(0, MAX_EVENT_ENTRIES) },
    files,
    "diagnostic",
    "application/json",
  );
  await writeJsonArtifact(
    outputDirectory,
    "network-failures.json",
    { entries: allNetworkFailures.slice(0, MAX_EVENT_ENTRIES) },
    files,
    "diagnostic",
    "application/json",
  );

  const finishedAt = new Date().toISOString();
  const passed = results.filter(({ outcome }) => outcome === "passed").length;
  const failed = results.filter(({ outcome }) => outcome === "failed").length;
  const skipped = results.filter(({ outcome }) => outcome === "skipped").length;
  const outcome =
    failed === 0 && !environment.interrupted ? "passed" : "failed";
  // Automation-only corrections are recorded per kind so an assisted pass is
  // never mistaken for ordinary-browser parity (#141).
  const corrections = [
    {
      kind: "host-lan-asset-mirror",
      applied: Boolean(assetMirrorUrl),
      interceptedRequests: mirrorCounters.intercepted,
    },
  ];
  const browserParity = corrections.some((correction) => correction.applied)
    ? "assisted"
    : "unmodified";
  const concurrencyReport = {
    requestedWorkers: concurrencyPolicy.workers,
    effectiveWorkers: concurrency.workers,
    limitedBy: concurrency.limitedBy,
    maxObservedParallel,
    ...(concurrencyPolicy.testTimeoutMilliseconds === null
      ? {}
      : {
          testTimeoutMilliseconds: concurrencyPolicy.testTimeoutMilliseconds,
        }),
    sessionRole,
    groups: describeConcurrencyGroups(plans),
  };
  const summary = {
    schemaVersion: SCHEMA_VERSION,
    sessionId: request.sessionId,
    outcome,
    passed,
    failed,
    skipped,
    startedAt,
    finishedAt,
    browserName: "chromium",
    browserVersion,
    playwrightVersion,
    browserParity,
    corrections,
    concurrency: concurrencyReport,
    tests: results,
    ...(environment.report ? { environment: environment.report } : {}),
  };
  await writeJsonArtifact(
    outputDirectory,
    "summary.json",
    summary,
    files,
    "browser-report",
    "application/json",
  );
  await writeTextArtifact(
    outputDirectory,
    "report.html",
    renderHtmlReport(suite.name, summary),
    files,
    "browser-report",
    "text/html; charset=utf-8",
  );

  await enforceOutputLimits(outputDirectory, request.policy.maxArtifactBytes);
  await sanitizeArtifacts(outputDirectory, secrets);
  const describedFiles = await describeFiles(outputDirectory, files);
  const manifest = {
    schemaVersion: SCHEMA_VERSION,
    sessionId: request.sessionId,
    createdAt: finishedAt,
    hostPlatform: request.runtimeContext.hostPlatform,
    studioPlatform: request.runtimeContext.studioPlatform,
    ...(request.runtimeContext.runtimePlatform
      ? { runtimePlatform: request.runtimeContext.runtimePlatform }
      : {}),
    backend: request.runtimeContext.backend,
    runtimeMode: request.runtimeContext.runtimeMode,
    ...(request.runtimeContext.studioVersion
      ? { studioVersion: request.runtimeContext.studioVersion }
      : {}),
    ...(request.runtimeContext.runtimeVersion
      ? { runtimeVersion: request.runtimeContext.runtimeVersion }
      : {}),
    browser: { name: "chromium", version: browserVersion },
    playwrightVersion,
    runnerVersion: RUNNER_VERSION,
    suite: {
      name: redactText(suite.name, secrets),
      tests: suite.tests.length,
    },
    policy: request.policy,
    corrections,
    concurrency: concurrencyReport,
    artifacts: describedFiles,
    ...(environment.report ? { environment: environment.report } : {}),
  };
  await writeTextFile(
    path.join(outputDirectory, "artifact-manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  files.push({
    path: "artifact-manifest.json",
    kind: "browser-report",
    mediaType: "application/json",
  });
  await verifyNoSecrets(outputDirectory, secrets);
  await enforceOutputLimits(outputDirectory, request.policy.maxArtifactBytes);
  return { ...summary, files };
}

async function runTest({
  environment,
  browser,
  baseUrl,
  assetMirrorUrl,
  mirrorCounters,
  index,
  outputDirectory,
  policy,
  secrets,
  signal,
  storageState,
  suite,
  test,
}) {
  const files = [];
  const ordinal = String(index + 1).padStart(3, "0");
  const harPath = policy.recordHar
    ? path.join(outputDirectory, `test-${ordinal}.har`)
    : undefined;
  const videoDirectory = policy.recordVideo
    ? path.join(outputDirectory, `video-${ordinal}`)
    : undefined;
  if (videoDirectory) await fsp.mkdir(videoDirectory, { mode: 0o700 });
  const contextOptions = {
    baseURL: baseUrl.origin,
    serviceWorkers: "block",
    ...(storageState ? { storageState } : {}),
    ...(harPath
      ? {
          recordHar: {
            path: harPath,
            mode: "minimal",
            content: "omit",
          },
        }
      : {}),
    ...(videoDirectory
      ? {
          recordVideo: {
            dir: videoDirectory,
            size: { width: 1280, height: 720 },
          },
        }
      : {}),
  };
  const context = await browser.newContext(contextOptions);
  if (assetMirrorUrl) {
    await installHostLanAssetRoute(context, assetMirrorUrl, mirrorCounters);
  }
  await context.addInitScript((style) => {
    const apply = () => {
      if (globalThis.document.getElementById("mendimaru-private-style")) return;
      const element = globalThis.document.createElement("style");
      element.id = "mendimaru-private-style";
      element.textContent = style;
      (globalThis.document.head || globalThis.document.documentElement).append(
        element,
      );
    };
    if (globalThis.document.readyState === "loading") {
      globalThis.document.addEventListener("DOMContentLoaded", apply, {
        once: true,
      });
    } else {
      apply();
    }
  }, privateStyle(suite.maskLocators));
  await context.tracing.start({
    screenshots: false,
    snapshots: true,
    sources: false,
    title: `Mendimaru browser test ${ordinal}`,
  });
  const page = await context.newPage();
  page.setDefaultNavigationTimeout(policy.navigationTimeoutMilliseconds);
  page.setDefaultTimeout(policy.actionTimeoutMilliseconds);
  const consoleEntries = [];
  const pageErrors = [];
  const networkFailures = [];
  attachDiagnostics(
    page,
    consoleEntries,
    pageErrors,
    networkFailures,
    secrets,
    baseUrl,
  );
  let outcome = "passed";
  let failure = null;
  let invalidatedBy = null;
  let completedSteps = 0;
  try {
    for (const step of [...suite.beforeEach, ...test.steps]) {
      await cancellable(signal, () =>
        applyPrivateMasks(page, suite.maskLocators),
      );
      await cancellable(signal, () =>
        environment.step(() =>
          executeStep(page, baseUrl, step, policy, secrets),
        ),
      );
      assertSameOrigin(page, baseUrl);
      await cancellable(signal, () =>
        applyPrivateMasks(page, suite.maskLocators),
      );
      completedSteps += 1;
    }
    await cancellable(signal, () => page.waitForTimeout(100));
    assertSameOrigin(page, baseUrl);
    await applyPrivateMasks(page, suite.maskLocators);
    if (
      policy.failOnConsoleError &&
      consoleEntries.some(({ type }) => type === "error")
    ) {
      throw new RunnerError(
        "console_error_policy",
        "a console error violated the browser test policy",
      );
    }
    if (policy.failOnNetworkFailure && networkFailures.length > 0) {
      throw new RunnerError(
        "network_failure_policy",
        "a failed network request violated the browser test policy",
      );
    }
    if (pageErrors.length > 0) {
      throw new RunnerError(
        "page_error_policy",
        "an uncaught page error violated the browser test policy",
      );
    }
  } catch (error) {
    outcome = "failed";
    invalidatedBy = error?.invalidatedBy ?? null;
    failure = redactText(
      error instanceof Error ? error.message : "browser test failed",
      secrets,
    ).slice(0, MAX_TEXT_LENGTH);
    if (
      networkFailures.some(
        ({ diagnostic }) => diagnostic?.code === WIDGET_CSS_DIAGNOSTIC.code,
      )
    ) {
      const hint = ` ${WIDGET_CSS_DIAGNOSTIC.message} ${WIDGET_CSS_DIAGNOSTIC.documentation}`;
      failure = failure.slice(0, MAX_TEXT_LENGTH - hint.length) + hint;
    }
    await captureFailureArtifacts({
      context,
      files,
      ordinal,
      outputDirectory,
      page,
      secrets,
      suite,
    });
  }

  if (outcome === "failed") {
    const traceName = `test-${ordinal}-trace.zip`;
    await context.tracing
      .stop({ path: path.join(outputDirectory, traceName) })
      .catch(() => {});
    if (await directFileExists(path.join(outputDirectory, traceName))) {
      files.push({
        path: traceName,
        kind: "browser-trace",
        mediaType: "application/zip",
      });
    }
  } else {
    await context.tracing.stop().catch(() => {});
  }

  const video = page.video();
  await context.close().catch(() => {});
  if (harPath && (await directFileExists(harPath))) {
    files.push({
      path: path.basename(harPath),
      kind: "browser-report",
      mediaType: "application/json",
    });
  }
  if (video) {
    try {
      const source = await video.path();
      const destinationName = `test-${ordinal}.webm`;
      await fsp.rename(source, path.join(outputDirectory, destinationName));
      files.push({
        path: destinationName,
        kind: "diagnostic",
        mediaType: "video/webm",
      });
    } catch {
      // A failed or cancelled context may not produce a complete video.
    }
  }
  if (videoDirectory) {
    await fsp.rm(videoDirectory, { recursive: true, force: true });
  }
  return {
    summary: {
      name: test.name,
      outcome,
      completedSteps,
      totalSteps: suite.beforeEach.length + test.steps.length,
      ...(failure ? { failure } : {}),
      ...(invalidatedBy ? { invalidatedBy } : {}),
    },
    files,
    consoleEntries,
    pageErrors,
    networkFailures,
  };
}

/// Lazily launched Chromium processes, one per lane. Keeping the lanes
/// separate means a browser crash, a forced context teardown, or a cancelled
/// test in one lane cannot end another lane's run.
function createBrowserLanes(size) {
  const browsers = new Array(size).fill(null);
  return {
    async browser(lane) {
      const existing = browsers[lane];
      if (existing?.isConnected()) return existing;
      const launched = await chromium.launch({ headless: true });
      browsers[lane] = launched;
      return launched;
    },
    async close() {
      await Promise.all(
        browsers.map((browser) => browser?.close().catch(() => {})),
      );
      browsers.fill(null);
    },
  };
}

/// A lane that could not produce a result at all (for example a browser that
/// failed to launch for that lane) is recorded as an explicit failed test, not
/// as a silent pass and not as a whole-run abort.
function laneFailure(suite, index, outcome) {
  const test = suite.tests[index];
  const error = outcome?.failure;
  const invalidatedBy = error?.invalidatedBy ?? null;
  return {
    summary: {
      name: test.name,
      outcome: "failed",
      completedSteps: 0,
      totalSteps: suite.beforeEach.length + test.steps.length,
      failure: String(error?.message || "the browser worker failed").slice(
        0,
        MAX_TEXT_LENGTH,
      ),
      ...(invalidatedBy ? { invalidatedBy } : {}),
    },
    files: [],
    consoleEntries: [],
    pageErrors: [],
    networkFailures: [],
  };
}

function mapConcurrencyErrors(work) {
  try {
    return work();
  } catch (error) {
    if (error instanceof ConcurrencyError) {
      throw new RunnerError(error.code, error.message);
    }
    throw error;
  }
}

function availableParallelism() {
  try {
    return os.availableParallelism();
  } catch {
    return os.cpus().length || 1;
  }
}

/// Free memory plus a conservative share of the total, so a host that keeps a
/// large page cache is not treated as unable to run a second lane.
function availableMemoryBytes() {
  return Math.max(os.freemem(), Math.floor(os.totalmem() / 4));
}

function attachDiagnostics(
  page,
  consoleEntries,
  pageErrors,
  networkFailures,
  secrets,
  baseUrl,
) {
  page.on("console", (message) => {
    if (consoleEntries.length >= MAX_EVENT_ENTRIES) return;
    consoleEntries.push({
      type: message.type(),
      text: redactText(message.text(), secrets).slice(0, MAX_TEXT_LENGTH),
      location: safeUrl(message.location().url),
    });
  });
  page.on("pageerror", (error) => {
    if (pageErrors.length >= MAX_EVENT_ENTRIES) return;
    pageErrors.push({
      message: redactText(error.message, secrets).slice(0, MAX_TEXT_LENGTH),
    });
  });
  page.on("requestfailed", (request) => {
    if (networkFailures.length >= MAX_EVENT_ENTRIES) return;
    networkFailures.push({
      method: request.method(),
      url: safeUrl(request.url()),
      reason: redactText(
        request.failure()?.errorText || "request failed",
        secrets,
      ).slice(0, MAX_TEXT_LENGTH),
    });
  });
  page.on("response", (response) => {
    if (
      response.status() < 400 ||
      networkFailures.length >= MAX_EVENT_ENTRIES
    ) {
      return;
    }
    networkFailures.push({
      method: response.request().method(),
      url: safeUrl(response.url()),
      status: response.status(),
      reason: "http-error-status",
      ...(isMissingWidgetCss(response, baseUrl)
        ? { diagnostic: WIDGET_CSS_DIAGNOSTIC }
        : {}),
    });
  });
}

function isMissingWidgetCss(response, baseUrl) {
  const url = new URL(response.url());
  return (
    response.status() === 404 &&
    response.request().method() === "GET" &&
    response.request().resourceType() === "stylesheet" &&
    url.origin === baseUrl.origin &&
    url.pathname.endsWith("/dist/widgets.css")
  );
}

async function executeStep(page, baseUrl, step, policy, secrets) {
  if (step.action === "goto") {
    const target = new URL(step.path, baseUrl);
    if (target.origin !== baseUrl.origin) {
      throw new RunnerError(
        "cross_origin_navigation",
        "suite navigation must remain on the configured origin",
      );
    }
    await page.goto(target.href, {
      waitUntil: step.waitUntil || "domcontentloaded",
      timeout: policy.navigationTimeoutMilliseconds,
    });
    return;
  }
  if (step.action === "expectUrl") {
    const target = new URL(step.path, baseUrl);
    if (target.origin !== baseUrl.origin) {
      throw new RunnerError(
        "cross_origin_navigation",
        "suite URL assertions must remain on the configured origin",
      );
    }
    await expect(page).toHaveURL(target.href, {
      timeout: policy.assertionTimeoutMilliseconds,
    });
    return;
  }
  const locator = locate(page, step.locator);
  switch (step.action) {
    case "click":
      await locator.click({ timeout: policy.actionTimeoutMilliseconds });
      break;
    case "fill": {
      const value = resolveStepValue(step, secrets);
      if (step.sensitive !== false && step.valueFromEnv) {
        await locator.evaluate((element) =>
          element.setAttribute("data-mendimaru-private", "true"),
        );
      }
      await locator.fill(value, { timeout: policy.actionTimeoutMilliseconds });
      break;
    }
    case "check":
      await locator.check({ timeout: policy.actionTimeoutMilliseconds });
      break;
    case "uncheck":
      await locator.uncheck({ timeout: policy.actionTimeoutMilliseconds });
      break;
    case "selectOption":
      await locator.selectOption(step.value, {
        timeout: policy.actionTimeoutMilliseconds,
      });
      break;
    case "press":
      await locator.press(step.key, {
        timeout: policy.actionTimeoutMilliseconds,
      });
      break;
    case "expectVisible":
      await expect(locator).toBeVisible({
        timeout: policy.assertionTimeoutMilliseconds,
      });
      break;
    case "expectHidden":
      await expect(locator).toBeHidden({
        timeout: policy.assertionTimeoutMilliseconds,
      });
      break;
    case "expectText":
      await expect(locator).toHaveText(step.value, {
        timeout: policy.assertionTimeoutMilliseconds,
      });
      break;
    case "expectValue":
      await expect(locator).toHaveValue(step.value, {
        timeout: policy.assertionTimeoutMilliseconds,
      });
      break;
    default:
      throw new RunnerError(
        "invalid_suite",
        "unsupported browser suite action",
      );
  }
}

function assertSameOrigin(page, baseUrl) {
  let current;
  try {
    current = new URL(page.url());
  } catch {
    throw new RunnerError(
      "cross_origin_navigation",
      "browser navigation left the configured origin",
    );
  }
  if (current.origin !== baseUrl.origin) {
    throw new RunnerError(
      "cross_origin_navigation",
      "browser navigation left the configured origin",
    );
  }
}

function locate(page, specification) {
  switch (specification.by) {
    case "role":
      return page.getByRole(specification.role, {
        name: specification.name,
        exact: specification.exact !== false,
      });
    case "label":
      return page.getByLabel(specification.value, {
        exact: specification.exact !== false,
      });
    case "testId":
      return page.getByTestId(specification.value);
    case "mendixName":
      return page.locator(`.mx-name-${cssEscape(specification.value)}`);
    case "text":
      return page.getByText(specification.value, {
        exact: specification.exact !== false,
      });
    default:
      throw new RunnerError("invalid_suite", "unsupported locator strategy");
  }
}

async function applyPrivateMasks(page, specifications) {
  for (const specification of specifications) {
    await locate(page, specification)
      .evaluateAll((elements) => {
        for (const element of elements) {
          element.setAttribute("data-mendimaru-private", "true");
        }
      })
      .catch(() => {});
  }
}

function privateStyle(specifications) {
  const selectors = [];
  for (const specification of specifications) {
    if (specification.by === "mendixName") {
      selectors.push(`.mx-name-${cssEscape(specification.value)}`);
    } else if (
      specification.by === "testId" &&
      /^[A-Za-z0-9_-]+$/.test(specification.value)
    ) {
      selectors.push(`[data-testid="${specification.value}"]`);
    }
  }
  if (selectors.length === 0) return PRIVATE_STYLE;
  return `${PRIVATE_STYLE}\n${selectors.join(",\n")} {
    opacity: 0 !important;
    color: transparent !important;
    caret-color: transparent !important;
    text-shadow: none !important;
    -webkit-text-security: disc !important;
  }`;
}

function resolveStepValue(step, secrets) {
  if (step.valueFromEnv) {
    const value = process.env[step.valueFromEnv];
    if (!value) {
      throw new RunnerError(
        "auth_value_missing",
        "a required test authentication value is unavailable",
      );
    }
    if (step.sensitive !== false) secrets.add(value);
    return value;
  }
  return step.value;
}

async function captureFailureArtifacts({
  context,
  files,
  ordinal,
  outputDirectory,
  page,
  secrets,
  suite,
}) {
  const mask = [
    page.locator('input[type="password"]'),
    page.locator('[data-mendimaru-private="true"]'),
    page.locator(".mx-name-MendimaruPrivate"),
    ...suite.maskLocators.map((entry) => locate(page, entry)),
  ];
  const screenshotName = `test-${ordinal}-failure.png`;
  await page
    .screenshot({
      path: path.join(outputDirectory, screenshotName),
      fullPage: true,
      animations: "disabled",
      mask,
    })
    .then(() =>
      files.push({
        path: screenshotName,
        kind: "screenshot",
        mediaType: "image/png",
      }),
    )
    .catch(() => {});

  const domName = `test-${ordinal}-dom.html`;
  await page
    .content()
    .then((content) =>
      writeTextFile(
        path.join(outputDirectory, domName),
        redactText(content, secrets),
      ),
    )
    .then(() =>
      files.push({
        path: domName,
        kind: "dom-snapshot",
        mediaType: "text/html; charset=utf-8",
      }),
    )
    .catch(() => {});

  const accessibilityName = `test-${ordinal}-accessibility.json`;
  try {
    const session = await context.newCDPSession(page);
    const tree = await session.send("Accessibility.getFullAXTree");
    await session.detach();
    await writeTextFile(
      path.join(outputDirectory, accessibilityName),
      `${redactText(JSON.stringify(tree, null, 2), secrets)}\n`,
    );
    files.push({
      path: accessibilityName,
      kind: "ui-tree",
      mediaType: "application/json",
    });
  } catch {
    // A page that crashed during failure collection may not expose CDP data.
  }
}

function validateRequest(value) {
  assertPlainObject(value, "invalid browser runner request");
  assertAllowedKeys(
    value,
    [
      "schemaVersion",
      "sessionId",
      "baseUrl",
      "assetMirrorUrl",
      "environmentObserverUrl",
      "outputDirectory",
      "runtimeContext",
      "policy",
      "sessionRole",
      "suite",
    ],
    "invalid browser runner request",
  );
  if (value.schemaVersion !== SCHEMA_VERSION) {
    throw new RunnerError("contract_mismatch", "browser contract mismatch");
  }
  if (!/^session_[0-9a-f]{32}$/.test(value.sessionId)) {
    throw new RunnerError(
      "invalid_request",
      "invalid browser session identity",
    );
  }
  observerUrl(value.environmentObserverUrl);
  validateRuntimeContext(value.runtimeContext);
  mapConcurrencyErrors(() => normalizeSessionRole(value.sessionRole));
  validatePolicy(value.policy);
  if (value.assetMirrorUrl !== undefined) {
    validateOptionalAssetMirrorUrl(value.assetMirrorUrl);
  }
  return value;
}

function validateOptionalAssetMirrorUrl(value) {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string") {
    throw new RunnerError(
      "invalid_request",
      "invalid browser asset mirror URL",
    );
  }
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new RunnerError(
      "invalid_request",
      "invalid browser asset mirror URL",
    );
  }
  if (
    url.protocol !== "http:" ||
    url.hostname !== "127.0.0.1" ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  ) {
    throw new RunnerError(
      "invalid_request",
      "invalid browser asset mirror URL",
    );
  }
  return url;
}

async function installHostLanAssetRoute(context, mirrorUrl, counters) {
  await context.route(/^https?:\/\/host\.lan\/Data\//i, async (route) => {
    counters.intercepted += 1;
    const original = new URL(route.request().url());
    const mirrored = new URL(
      `${original.pathname}${original.search}`,
      mirrorUrl,
    );
    const response = await route.fetch({ url: mirrored.toString() });
    await route.fulfill({ response });
  });
}

function validateRuntimeContext(value) {
  assertPlainObject(value, "invalid runtime context");
  assertAllowedKeys(
    value,
    [
      "hostPlatform",
      "studioPlatform",
      "runtimePlatform",
      "backend",
      "runtimeMode",
      "studioVersion",
      "runtimeVersion",
    ],
    "invalid runtime context",
  );
  assertEnum(value.hostPlatform, ["linux", "windows", "macos", "unsupported"]);
  assertEnum(value.studioPlatform, [
    "linux",
    "windows",
    "macos",
    "unsupported",
  ]);
  if (value.runtimePlatform !== undefined) {
    assertEnum(value.runtimePlatform, ["linux", "windows", "macos"]);
  }
  assertEnum(value.backend, ["linux-winboat", "windows-native", "mac-native"]);
  assertEnum(value.runtimeMode, [
    "portable",
    "studio-run-locally",
    "external-url",
  ]);
  for (const key of ["studioVersion", "runtimeVersion"]) {
    if (value[key] !== undefined) assertBoundedString(value[key], 1, 80);
  }
}

function validatePolicy(value) {
  assertPlainObject(value, "invalid browser policy");
  const required = [
    "navigationTimeoutMilliseconds",
    "actionTimeoutMilliseconds",
    "assertionTimeoutMilliseconds",
    "failOnConsoleError",
    "failOnNetworkFailure",
    "recordVideo",
    "recordHar",
    "maxArtifactBytes",
    "retentionRuns",
  ];
  assertAllowedKeys(
    value,
    [...required, "concurrency"],
    "invalid browser policy",
  );
  assertRequiredKeys(value, required, "invalid browser policy");
  for (const key of [
    "navigationTimeoutMilliseconds",
    "actionTimeoutMilliseconds",
    "assertionTimeoutMilliseconds",
  ]) {
    assertInteger(value[key], 100, 300_000);
  }
  for (const key of [
    "failOnConsoleError",
    "failOnNetworkFailure",
    "recordVideo",
    "recordHar",
  ]) {
    if (typeof value[key] !== "boolean") {
      throw new RunnerError("invalid_request", "invalid browser policy flag");
    }
  }
  assertInteger(value.maxArtifactBytes, 1_048_576, 536_870_912);
  assertInteger(value.retentionRuns, 1, 100);
  mapConcurrencyErrors(() => normalizeConcurrencyPolicy(value.concurrency));
}

function validateSuite(value, sessionRole = "owner") {
  assertPlainObject(value, "invalid browser suite");
  assertAllowedKeys(
    value,
    [
      "schemaVersion",
      "name",
      "beforeEach",
      "tests",
      "maskLocators",
      "secretEnv",
      "storageStateEnv",
    ],
    "invalid browser suite",
  );
  if (value.schemaVersion !== "1.0.0") {
    throw new RunnerError("invalid_suite", "unsupported browser suite version");
  }
  assertBoundedString(value.name, 1, 160);
  if (!Array.isArray(value.tests) || value.tests.length === 0) {
    throw new RunnerError("invalid_suite", "browser suite has no tests");
  }
  if (value.tests.length > MAX_SUITE_TESTS) {
    throw new RunnerError("invalid_suite", "browser suite has too many tests");
  }
  const beforeEach = value.beforeEach || [];
  const maskLocators = value.maskLocators || [];
  const secretEnv = value.secretEnv || [];
  if (!Array.isArray(beforeEach) || beforeEach.length > MAX_STEPS_PER_TEST) {
    throw new RunnerError("invalid_suite", "invalid browser beforeEach steps");
  }
  if (!Array.isArray(maskLocators) || maskLocators.length > 50) {
    throw new RunnerError("invalid_suite", "invalid browser mask locators");
  }
  if (!Array.isArray(secretEnv) || secretEnv.length > 50) {
    throw new RunnerError("invalid_suite", "invalid browser secret list");
  }
  beforeEach.forEach(validateStep);
  maskLocators.forEach(validateLocator);
  secretEnv.forEach(validateEnvironmentName);
  let storageStatePath;
  if (value.storageStateEnv !== undefined) {
    validateEnvironmentName(value.storageStateEnv);
    storageStatePath = process.env[value.storageStateEnv];
    if (!storageStatePath) {
      throw new RunnerError(
        "auth_value_missing",
        "the authentication storage state is unavailable",
      );
    }
    if (!path.isAbsolute(storageStatePath)) {
      throw new RunnerError(
        "invalid_suite",
        "the authentication storage state path must be absolute",
      );
    }
  }
  const tests = value.tests.map((test) => {
    assertPlainObject(test, "invalid browser test");
    assertAllowedKeys(
      test,
      ["name", "steps", "concurrency"],
      "invalid browser test",
    );
    assertRequiredKeys(test, ["name", "steps"], "invalid browser test");
    assertBoundedString(test.name, 1, 160);
    if (!Array.isArray(test.steps) || test.steps.length > MAX_STEPS_PER_TEST) {
      throw new RunnerError("invalid_suite", "invalid browser test steps");
    }
    test.steps.forEach(validateStep);
    const concurrency = mapConcurrencyErrors(() =>
      normalizeTestConcurrency(test.concurrency, { sessionRole }),
    );
    return { name: test.name, steps: test.steps, concurrency };
  });
  return {
    name: value.name,
    beforeEach,
    tests,
    maskLocators,
    secretEnv,
    storageStateEnv: value.storageStateEnv,
    storageStatePath,
  };
}

function validateStep(step) {
  assertPlainObject(step, "invalid browser test step");
  const actions = [
    "goto",
    "click",
    "fill",
    "check",
    "uncheck",
    "selectOption",
    "press",
    "expectVisible",
    "expectHidden",
    "expectText",
    "expectValue",
    "expectUrl",
  ];
  assertEnum(step.action, actions);
  if (step.action === "goto") {
    assertAllowedKeys(
      step,
      ["action", "path", "waitUntil"],
      "invalid goto step",
    );
    assertBoundedString(step.path, 1, 2_048);
    if (step.waitUntil !== undefined) {
      assertEnum(step.waitUntil, [
        "commit",
        "domcontentloaded",
        "load",
        "networkidle",
      ]);
    }
    return;
  }
  if (step.action === "expectUrl") {
    assertExactKeys(step, ["action", "path"], "invalid URL assertion step");
    assertBoundedString(step.path, 1, 2_048);
    return;
  }
  validateLocator(step.locator);
  if (step.action === "fill") {
    assertAllowedKeys(
      step,
      ["action", "locator", "value", "valueFromEnv", "sensitive"],
      "invalid fill step",
    );
    if ((step.value === undefined) === (step.valueFromEnv === undefined)) {
      throw new RunnerError(
        "invalid_suite",
        "fill requires exactly one value source",
      );
    }
    if (step.value !== undefined) assertBoundedString(step.value, 0, 8_192);
    if (step.value !== undefined && step.sensitive !== undefined) {
      throw new RunnerError(
        "invalid_suite",
        "literal browser values cannot be marked as secret",
      );
    }
    if (step.valueFromEnv !== undefined)
      validateEnvironmentName(step.valueFromEnv);
    if (step.sensitive !== undefined && typeof step.sensitive !== "boolean") {
      throw new RunnerError("invalid_suite", "invalid sensitive flag");
    }
    return;
  }
  if (["selectOption", "expectText", "expectValue"].includes(step.action)) {
    assertExactKeys(step, ["action", "locator", "value"], "invalid value step");
    assertBoundedString(step.value, 0, 8_192);
    return;
  }
  if (step.action === "press") {
    assertExactKeys(step, ["action", "locator", "key"], "invalid key step");
    assertBoundedString(step.key, 1, 80);
    return;
  }
  assertExactKeys(step, ["action", "locator"], "invalid browser step");
}

function validateLocator(value) {
  assertPlainObject(value, "invalid browser locator");
  assertEnum(value.by, ["role", "label", "testId", "mendixName", "text"]);
  if (value.by === "role") {
    assertAllowedKeys(
      value,
      ["by", "role", "name", "exact"],
      "invalid role locator",
    );
    assertBoundedString(value.role, 1, 80);
    assertBoundedString(value.name, 1, 500);
  } else {
    assertAllowedKeys(
      value,
      ["by", "value", "exact"],
      "invalid browser locator",
    );
    assertBoundedString(value.value, 1, 500);
    if (value.by === "mendixName" && !/^[A-Za-z0-9_-]+$/.test(value.value)) {
      throw new RunnerError("invalid_suite", "invalid Mendix widget name");
    }
  }
  if (value.exact !== undefined && typeof value.exact !== "boolean") {
    throw new RunnerError("invalid_suite", "invalid locator exact flag");
  }
}

async function collectSecrets(suite) {
  const secrets = new Set();
  let storageState;
  for (const name of suite.secretEnv) {
    const value = process.env[name];
    if (!value) {
      throw new RunnerError(
        "auth_value_missing",
        "a declared test secret is unavailable",
      );
    }
    secrets.add(value);
  }
  for (const step of [
    ...suite.beforeEach,
    ...suite.tests.flatMap(({ steps }) => steps),
  ]) {
    if (step.valueFromEnv && step.sensitive !== false) {
      const value = process.env[step.valueFromEnv];
      if (!value) {
        throw new RunnerError(
          "auth_value_missing",
          "a required test authentication value is unavailable",
        );
      }
      secrets.add(value);
    }
  }
  if (suite.storageStatePath) {
    storageState = await readDirectJson(
      suite.storageStatePath,
      2 * 1024 * 1024,
    );
    collectStorageSecrets(storageState, secrets);
  }
  return { secrets, storageState };
}

function collectStorageSecrets(state, result) {
  for (const cookie of Array.isArray(state.cookies) ? state.cookies : []) {
    if (typeof cookie?.value === "string" && cookie.value) {
      result.add(cookie.value);
    }
  }
  for (const origin of Array.isArray(state.origins) ? state.origins : []) {
    const entries = Array.isArray(origin?.localStorage)
      ? origin.localStorage
      : [];
    for (const entry of entries) {
      if (typeof entry?.value === "string" && entry.value) {
        result.add(entry.value);
      }
    }
  }
}

async function describeFiles(directory, files) {
  const descriptions = [];
  for (const file of files) {
    const filename = safeArtifactPath(directory, file.path);
    const { sha256, sizeBytes } = await digestFile(filename);
    descriptions.push({
      file: file.path,
      kind: file.kind,
      mediaType: file.mediaType,
      sha256,
      sizeBytes,
    });
  }
  return descriptions;
}

async function sanitizeArtifacts(directory, secrets) {
  const variants = secretVariants(secrets);
  const needles = variants.map((value) => strToU8(value));
  const matcher = new BytePatternMatcher(needles);
  for (const entry of await fsp.readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) continue;
    if (!entry.isFile() || entry.isSymbolicLink()) {
      throw new RunnerError("artifact_unsafe", "unsafe browser artifact entry");
    }
    const filename = path.join(directory, entry.name);
    if (entry.name.endsWith(".zip")) {
      let archive;
      try {
        archive = unzipArchiveBounded(await fsp.readFile(filename), {
          collect: secrets.size !== 0,
        });
      } catch (error) {
        throwArtifactSafetyError(error);
      }
      if (secrets.size === 0) continue;
      for (const [name, bytes] of Object.entries(archive)) {
        if (matcher.contains(Buffer.from(name, "utf8"))) {
          throw new RunnerError(
            "artifact_secret_detected",
            "a trace entry name contains a private value",
          );
        }
        archive[name] = sanitizeBytes(bytes, matcher);
      }
      await fsp.writeFile(filename, zipSync(archive, { level: 6 }), {
        mode: 0o600,
      });
    } else if (
      secrets.size !== 0 &&
      !entry.name.endsWith(".png") &&
      !entry.name.endsWith(".webm")
    ) {
      await sanitizeFileStreaming(filename, matcher);
    }
  }
}

function sanitizeBytes(bytes, matcher) {
  return matcher.redact(bytes);
}

async function verifyNoSecrets(directory, secrets) {
  if (secrets.size === 0) return;
  const variants = secretVariants(secrets).map((value) => strToU8(value));
  const matcher = new BytePatternMatcher(variants);
  const startedAt = Date.now();
  for (const entry of await fsp.readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) continue;
    const filename = path.join(directory, entry.name);
    try {
      if (entry.name.endsWith(".zip")) {
        unzipArchiveBounded(await fsp.readFile(filename), {
          collect: false,
          needles: variants,
          startedAt,
        });
      } else {
        await scanFileForSecrets(filename, matcher, startedAt);
      }
    } catch (error) {
      throwArtifactSafetyError(error);
    }
  }
}

async function digestFile(filename) {
  const digest = createHash("sha256");
  let sizeBytes = 0;
  for await (const chunk of fs.createReadStream(filename, {
    highWaterMark: ARTIFACT_SCAN_BUFFER_BYTES,
  })) {
    sizeBytes += chunk.length;
    digest.update(chunk);
  }
  return { sha256: digest.digest("hex"), sizeBytes };
}

async function scanFileForSecrets(filename, matcher, startedAt) {
  const scanner = new StreamingPatternScanner(matcher);
  let actualBytes = 0;
  for await (const chunk of fs.createReadStream(filename, {
    highWaterMark: ARTIFACT_SCAN_BUFFER_BYTES,
  })) {
    if (
      Date.now() - startedAt >=
      DEFAULT_ARTIFACT_SAFETY_LIMITS.maximumDurationMilliseconds
    ) {
      throw new ArtifactSafetyError("scan_time_limit");
    }
    actualBytes += chunk.length;
    if (actualBytes > DEFAULT_ARTIFACT_SAFETY_LIMITS.maximumFileBytes) {
      throw new ArtifactSafetyError("file_size_limit");
    }
    scanner.push(chunk);
  }
}

async function sanitizeFileStreaming(filename, matcher) {
  const suffix = randomBytes(12).toString("hex");
  const temporary = `${filename}.${suffix}.redact`;
  const maximumOverlap = Math.max(0, matcher.maximumPatternBytes - 1);
  let source;
  let destination;
  try {
    source = await fsp.open(filename, "r");
    destination = await fsp.open(temporary, "wx", 0o600);
    const buffer = Buffer.allocUnsafe(ARTIFACT_SCAN_BUFFER_BYTES);
    let pending = Buffer.alloc(0);
    let actualBytes = 0;
    const startedAt = Date.now();
    while (true) {
      const { bytesRead } = await source.read(buffer, 0, buffer.length, null);
      if (bytesRead === 0) break;
      actualBytes += bytesRead;
      if (
        actualBytes > DEFAULT_ARTIFACT_SAFETY_LIMITS.maximumFileBytes ||
        Date.now() - startedAt >=
          DEFAULT_ARTIFACT_SAFETY_LIMITS.maximumDurationMilliseconds
      ) {
        throw new ArtifactSafetyError(
          actualBytes > DEFAULT_ARTIFACT_SAFETY_LIMITS.maximumFileBytes
            ? "file_size_limit"
            : "scan_time_limit",
        );
      }
      const combined = matcher.redact(
        Buffer.concat([pending, buffer.subarray(0, bytesRead)]),
      );
      const retained = Math.min(maximumOverlap, combined.length);
      const writable = combined.length - retained;
      if (writable !== 0) {
        await writeAll(destination, combined.subarray(0, writable));
      }
      pending = Buffer.from(combined.subarray(writable));
    }
    if (pending.length !== 0) await writeAll(destination, pending);
    await destination.sync();
    await source.close();
    source = undefined;
    await destination.close();
    destination = undefined;
    await fsp.rename(temporary, filename);
    if (process.platform !== "win32") await fsp.chmod(filename, 0o600);
  } catch (error) {
    if (error instanceof ArtifactSafetyError) throwArtifactSafetyError(error);
    throw error;
  } finally {
    await source?.close().catch(() => {});
    await destination?.close().catch(() => {});
    await fsp.rm(temporary, { force: true }).catch(() => {});
  }
}

async function writeAll(file, bytes) {
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesWritten } = await file.write(
      bytes,
      offset,
      bytes.length - offset,
    );
    if (bytesWritten === 0) throw new Error("browser artifact write stalled");
    offset += bytesWritten;
  }
}

function throwArtifactSafetyError(error) {
  if (!(error instanceof ArtifactSafetyError)) throw error;
  if (error.kind === "private_entry_name" || error.kind === "private_value") {
    throw new RunnerError("artifact_secret_detected", error.message);
  }
  if (error.kind.endsWith("_limit")) {
    throw new RunnerError("artifact_limit_exceeded", error.message);
  }
  throw new RunnerError("artifact_unsafe", error.message);
}

function redactText(value, secrets) {
  let output = String(value);
  for (const secret of secretVariants(secrets)) {
    output = output.split(secret).join("[REDACTED]");
  }
  return output;
}

function secretVariants(secrets) {
  const values = new Set();
  for (const secret of secrets) {
    if (!secret) continue;
    values.add(secret);
    const percentEncoded = encodeURIComponent(secret);
    values.add(percentEncoded);
    values.add(percentEncoded.replaceAll("%20", "+"));
    const lowercasePercent = percentEncoded.replace(/%[0-9A-F]{2}/g, (value) =>
      value.toLowerCase(),
    );
    values.add(lowercasePercent);
    values.add(lowercasePercent.replaceAll("%20", "+"));
    values.add(JSON.stringify(secret).slice(1, -1));
    values.add(htmlEscape(secret, false));
    values.add(htmlEscape(secret, true));
    const base64 = Buffer.from(secret, "utf8").toString("base64");
    values.add(base64);
    values.add(base64.replaceAll("+", "-").replaceAll("/", "_"));
    values.add(Buffer.from(secret, "utf8").toString("base64url"));
  }
  return [...values]
    .filter(Boolean)
    .sort((left, right) => right.length - left.length);
}

function htmlEscape(value, quote) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', quote ? "&quot;" : '"')
    .replaceAll("'", quote ? "&#39;" : "'");
}

async function enforceOutputLimits(directory, maximumBytes) {
  let total = 0;
  let count = 0;
  for (const entry of await fsp.readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      throw new RunnerError(
        "artifact_unsafe",
        "nested artifact directory remained",
      );
    }
    if (!entry.isFile() || entry.isSymbolicLink()) {
      throw new RunnerError("artifact_unsafe", "unsafe browser artifact entry");
    }
    if (!/^[A-Za-z0-9._-]{1,120}$/.test(entry.name)) {
      throw new RunnerError(
        "artifact_unsafe",
        "unsafe browser artifact filename",
      );
    }
    const metadata = await fsp.lstat(path.join(directory, entry.name));
    total += metadata.size;
    count += 1;
    if (total > maximumBytes || count > 512) {
      throw new RunnerError(
        "artifact_limit_exceeded",
        "browser artifact limit exceeded",
      );
    }
    if (process.platform !== "win32") {
      await fsp.chmod(path.join(directory, entry.name), 0o600);
    }
  }
}

async function validateOutputDirectory(value) {
  if (typeof value !== "string" || !path.isAbsolute(value)) {
    throw new RunnerError(
      "invalid_request",
      "invalid browser output directory",
    );
  }
  const metadata = await fsp.lstat(value).catch(() => null);
  if (!metadata?.isDirectory() || metadata.isSymbolicLink()) {
    throw new RunnerError("artifact_unsafe", "unsafe browser output directory");
  }
  const entries = await fsp.readdir(value);
  if (entries.length !== 0) {
    throw new RunnerError(
      "artifact_unsafe",
      "browser output directory is not empty",
    );
  }
  if (process.platform !== "win32") await fsp.chmod(value, 0o700);
  return path.resolve(value);
}

function validateBaseUrl(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new RunnerError("invalid_request", "invalid browser base URL");
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new RunnerError("invalid_request", "unsafe browser base URL");
  }
  return url;
}

async function readRequest() {
  const chunks = [];
  let length = 0;
  for await (const chunk of process.stdin) {
    length += chunk.length;
    if (length > MAX_STDIN_BYTES) {
      throw new RunnerError(
        "invalid_request",
        "browser runner request is too large",
      );
    }
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new RunnerError(
      "invalid_request",
      "browser runner request is not JSON",
    );
  }
}

async function readDirectJson(filename, maximumBytes) {
  const flags =
    process.platform === "win32"
      ? fs.constants.O_RDONLY
      : fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW;
  const handle = await fsp.open(filename, flags).catch(() => null);
  if (!handle) {
    throw new RunnerError(
      "invalid_suite",
      "unsafe authentication storage state",
    );
  }
  try {
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size > maximumBytes) {
      throw new RunnerError(
        "invalid_suite",
        "unsafe authentication storage state",
      );
    }
    return JSON.parse(await handle.readFile("utf8"));
  } catch {
    throw new RunnerError(
      "invalid_suite",
      "invalid authentication storage state",
    );
  } finally {
    await handle.close().catch(() => {});
  }
}

async function directFileExists(filename) {
  const metadata = await fsp.lstat(filename).catch(() => null);
  return Boolean(metadata?.isFile() && !metadata.isSymbolicLink());
}

async function writeJsonArtifact(
  directory,
  name,
  value,
  files,
  kind,
  mediaType,
) {
  await writeTextArtifact(
    directory,
    name,
    `${JSON.stringify(value, null, 2)}\n`,
    files,
    kind,
    mediaType,
  );
}

async function writeTextArtifact(
  directory,
  name,
  value,
  files,
  kind,
  mediaType,
) {
  await writeTextFile(path.join(directory, name), value);
  files.push({ path: name, kind, mediaType });
}

async function writeTextFile(filename, value) {
  await fsp.writeFile(filename, value, { encoding: "utf8", mode: 0o600 });
}

function safeArtifactPath(directory, relativePath) {
  if (!/^[A-Za-z0-9._-]{1,120}$/.test(relativePath)) {
    throw new RunnerError("artifact_unsafe", "unsafe browser artifact path");
  }
  const resolved = path.resolve(directory, relativePath);
  if (path.dirname(resolved) !== path.resolve(directory)) {
    throw new RunnerError("artifact_unsafe", "unsafe browser artifact path");
  }
  return resolved;
}

function safeUrl(value) {
  if (!value) return "";
  try {
    const url = new URL(value);
    url.username = "";
    url.password = "";
    url.search = "";
    url.hash = "";
    return url.href;
  } catch {
    return "invalid-url";
  }
}

function renderHtmlReport(suiteName, summary) {
  const rows = summary.tests
    .map(
      (test) =>
        `<tr><td>${escapeHtml(test.name)}</td><td>${escapeHtml(
          test.outcome,
        )}</td><td>${test.completedSteps}/${test.totalSteps}</td><td>${escapeHtml(
          test.invalidatedBy
            ? `[${test.invalidatedBy}] ${test.failure || ""}`.trim()
            : test.failure || "",
        )}</td></tr>`,
    )
    .join("");
  const correctionText = (summary.corrections || [])
    .map(
      (correction) =>
        `${escapeHtml(correction.kind)}: ${correction.applied ? "applied" : "not applied"} (${correction.interceptedRequests} intercepted)`,
    )
    .join(" · ");
  const parityLine = summary.browserParity
    ? `<p>Browser parity: <strong>${escapeHtml(summary.browserParity)}</strong>${correctionText ? ` — ${correctionText}` : ""}</p>`
    : "";
  const concurrency = summary.concurrency;
  const concurrencyLine = concurrency
    ? `<p>Workers: <strong>${concurrency.effectiveWorkers}</strong> of ${concurrency.requestedWorkers} requested (limited by ${escapeHtml(
        concurrency.limitedBy,
      )}), peak parallel ${concurrency.maxObservedParallel}, session role ${escapeHtml(
        concurrency.sessionRole,
      )}${
        concurrency.groups.length > 0
          ? ` — ${concurrency.groups
              .map(
                (group) =>
                  `${escapeHtml(group.resource)}: ${group.tests} ${escapeHtml(
                    group.mode,
                  )}`,
              )
              .join(" · ")}`
          : ""
      }</p>`
    : "";
  return `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Mendimaru browser report</title>
<style>body{font:14px system-ui;margin:2rem;color:#17202a}table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccd1d1;padding:.5rem;text-align:left}th{background:#f4f6f7}.passed{color:#196f3d}.failed{color:#922b21}.assisted{color:#b9770e}</style></head>
<body><h1>${escapeHtml(suiteName)}</h1><p class="${summary.outcome}">Outcome: ${summary.outcome}</p>
${parityLine}
${concurrencyLine}
<p>Chromium ${escapeHtml(summary.browserVersion)} · Playwright ${escapeHtml(
    summary.playwrightVersion,
  )}</p><table><thead><tr><th>Test</th><th>Outcome</th><th>Steps</th><th>Failure</th></tr></thead><tbody>${rows}</tbody></table></body></html>\n`;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function cssEscape(value) {
  return value.replace(
    /[^A-Za-z0-9_-]/g,
    (character) => `\\${character.codePointAt(0).toString(16)} `,
  );
}

function assertPlainObject(value, message) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new RunnerError("invalid_suite", message);
  }
}

function assertAllowedKeys(value, keys, message) {
  const allowed = new Set(keys);
  if (Object.keys(value).some((key) => !allowed.has(key))) {
    throw new RunnerError("invalid_suite", message);
  }
}

function assertExactKeys(value, keys, message) {
  assertAllowedKeys(value, keys, message);
  assertRequiredKeys(value, keys, message);
}

function assertRequiredKeys(value, keys, message) {
  if (keys.some((key) => !(key in value))) {
    throw new RunnerError("invalid_suite", message);
  }
}

function assertEnum(value, options) {
  if (!options.includes(value)) {
    throw new RunnerError("invalid_suite", "invalid browser suite enum value");
  }
}

function assertInteger(value, minimum, maximum) {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new RunnerError("invalid_request", "invalid browser policy number");
  }
}

function assertBoundedString(value, minimum, maximum) {
  if (
    typeof value !== "string" ||
    value.length < minimum ||
    value.length > maximum
  ) {
    throw new RunnerError("invalid_suite", "invalid browser suite string");
  }
}

function validateEnvironmentName(value) {
  if (
    typeof value !== "string" ||
    !/^MENDIMARU_TEST_[A-Z0-9_]{1,64}$/.test(value)
  ) {
    throw new RunnerError(
      "invalid_suite",
      "invalid test environment variable name",
    );
  }
}

function safeRunnerMessage(code) {
  const messages = {
    invalid_request: "the browser runner request is invalid",
    invalid_suite: "the browser suite is invalid",
    concurrency_policy_refused:
      "the browser suite conflicts with the requested parallel execution policy",
    contract_mismatch: "the browser runner contract does not match",
    chromium_unavailable: "the pinned Playwright Chromium build is unavailable",
    chromium_install_failed: "the explicit Chromium installation failed",
    node_unsupported: "the host Node.js version is unsupported",
    auth_value_missing:
      "a required browser test authentication value is unavailable",
    cross_origin_navigation:
      "browser suite navigation crossed the configured origin",
    artifact_unsafe: "the browser artifact destination is unsafe",
    artifact_limit_exceeded:
      "the browser artifacts exceeded the configured limit",
    artifact_secret_detected: "a private value remained in browser artifacts",
    runner_failed: "the browser runner failed",
  };
  return messages[code] || messages.runner_failed;
}

function requireSupportedNode() {
  if (!versionAtLeast(process.versions.node, MINIMUM_NODE_VERSION)) {
    throw new RunnerError(
      "node_unsupported",
      "the host Node.js version is unsupported",
    );
  }
}

function versionAtLeast(actual, required) {
  const actualParts = String(actual).split(".").map(Number);
  const requiredParts = String(required).split(".").map(Number);
  if (
    actualParts.length < 3 ||
    actualParts.slice(0, 3).some((part) => !Number.isInteger(part))
  ) {
    return false;
  }
  for (let index = 0; index < 3; index += 1) {
    if (actualParts[index] !== requiredParts[index]) {
      return actualParts[index] > requiredParts[index];
    }
  }
  return true;
}
