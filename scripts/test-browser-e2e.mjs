import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import readline from "node:readline";
import { setTimeout } from "node:timers";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";
import {
  DEFAULT_ARTIFACT_SAFETY_LIMITS,
  inspectZipArchive,
} from "./browser-artifact-safety.mjs";
import {
  DEFAULT_WORKER_TEST_TIMEOUT_MILLISECONDS,
  MAXIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS,
  MAX_BROWSER_WORKERS,
  MINIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS,
} from "./browser-parallel.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const runner = path.join(repository, "scripts/browser-runner.mjs");
const fixtureServer = path.join(repository, "tests/browser/fixture-server.mjs");
const password = 'canary P@"ss&word</trace+har-2026';
const username = "fixture-user";
const MAX_COMPRESSIBLE_RUNNER_RSS_BYTES = 576 * 1024 * 1024;
const temporary = await fs.mkdtemp(
  path.join(os.tmpdir(), "mendimaru-browser-e2e-"),
);
const storageStatePath = path.join(temporary, "storage-state.json");
await fs.writeFile(
  storageStatePath,
  `${JSON.stringify({
    cookies: [
      {
        name: "fixture_auth",
        value: password,
        domain: "127.0.0.1",
        path: "/",
        expires: -1,
        httpOnly: true,
        secure: false,
        sameSite: "Lax",
      },
    ],
    origins: [],
  })}\n`,
  { mode: 0o600 },
);
const server = spawn(process.execPath, [fixtureServer], {
  cwd: repository,
  stdio: ["ignore", "pipe", "pipe"],
});

try {
  const { port } = await firstJsonLine(server.stdout);
  const baseUrl = `http://127.0.0.1:${port}/`;
  const doctor = await invokeRunner("doctor");
  assert.equal(
    doctor.ready,
    true,
    "Chromium must be launchable for strict E2E",
  );
  assert.equal(doctor.nodeSupported, true);
  assert.equal(doctor.minimumNodeVersion, "22.22.2");
  assert.match(doctor.playwrightVersion, /^\d+\.\d+\.\d+$/);
  assert.match(doctor.chromium.version, /^\d+/);

  const isolatedBrowsers = path.join(temporary, "missing-browsers");
  await fs.mkdir(isolatedBrowsers, { mode: 0o700 });
  const missingDoctor = await invokeRunner("doctor", undefined, {
    PLAYWRIGHT_BROWSERS_PATH: isolatedBrowsers,
  });
  assert.equal(missingDoctor.ready, false);
  assert.equal(missingDoctor.chromium.installed, false);
  assert.equal(missingDoctor.chromium.launchable, false);

  const missingOutput = path.join(temporary, "missing-browser-run");
  await fs.mkdir(missingOutput, { mode: 0o700 });
  const missingBrowser = await invokeRunnerFailure(
    "run",
    {
      schemaVersion: "5.0.0",
      sessionId: `session_${randomBytes(16).toString("hex")}`,
      baseUrl,
      outputDirectory: missingOutput,
      runtimeContext: {
        hostPlatform: "linux",
        studioPlatform: "windows",
        runtimePlatform: "linux",
        backend: "linux-winboat",
        runtimeMode: "portable",
        runtimeVersion: "11.12.2",
      },
      policy: {
        navigationTimeoutMilliseconds: 15_000,
        actionTimeoutMilliseconds: 5_000,
        assertionTimeoutMilliseconds: 1_000,
        failOnConsoleError: true,
        failOnNetworkFailure: true,
        recordVideo: false,
        recordHar: false,
        maxArtifactBytes: 128 * 1024 * 1024,
        retentionRuns: 20,
      },
      suite: JSON.parse(
        await fs.readFile(
          path.join(repository, "tests/browser/smoke.browser.json"),
          "utf8",
        ),
      ),
    },
    { PLAYWRIGHT_BROWSERS_PATH: isolatedBrowsers },
  );
  assert.equal(missingBrowser.error.code, "chromium_unavailable");
  assert.deepEqual(await fs.readdir(missingOutput), []);

  const portable = await runSuite({
    baseUrl,
    mode: "portable",
    name: "portable",
    suite: "smoke.browser.json",
  });
  assert.equal(portable.result.outcome, "passed");
  assert.equal(portable.result.passed, 1);
  assert.equal(portable.result.failed, 0);
  await verifyManifest(portable.directory, "portable", "linux");
  await verifyNoSecret(portable.directory);

  const storageAuth = await runSuite({
    baseUrl,
    mode: "portable",
    name: "storage-auth",
    suite: "storage-auth.browser.json",
  });
  assert.equal(storageAuth.result.outcome, "passed");
  await verifyNoSecret(storageAuth.directory);

  const winboat = await runSuite({
    baseUrl,
    mode: "studio-run-locally",
    name: "winboat",
    policy: { recordHar: true, recordVideo: true },
    runtimePlatform: "windows",
    suite: "smoke.browser.json",
  });
  assert.equal(winboat.result.outcome, "passed");
  assert(winboat.result.files.some(({ path: name }) => name.endsWith(".har")));
  assert(winboat.result.files.some(({ path: name }) => name.endsWith(".webm")));
  await verifyManifest(winboat.directory, "studio-run-locally", "windows");
  await verifyNoSecret(winboat.directory);

  const failure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "failure",
    suite: "failure.browser.json",
  });
  assert.equal(failure.result.outcome, "failed");
  assert.equal(failure.result.failed, 1);
  for (const suffix of [
    "-failure.png",
    "-dom.html",
    "-accessibility.json",
    "-trace.zip",
  ]) {
    assert(
      failure.result.files.some(({ path: name }) => name.endsWith(suffix)),
      `missing strict failure artifact ${suffix}`,
    );
  }
  await verifyNoSecret(failure.directory);

  const pageFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "page-error",
    suite: "page-error.browser.json",
  });
  assert.equal(pageFailure.result.outcome, "failed");
  const pageErrors = JSON.parse(
    await fs.readFile(
      path.join(pageFailure.directory, "page-errors.json"),
      "utf8",
    ),
  );
  assert(
    pageErrors.entries.some(({ message }) => /uncaught failure/.test(message)),
  );
  await verifyNoSecret(pageFailure.directory);

  const timeoutFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "navigation-timeout",
    policy: { navigationTimeoutMilliseconds: 100 },
    suite: "timeout.browser.json",
  });
  assert.equal(timeoutFailure.result.outcome, "failed");
  assert.match(timeoutFailure.result.tests[0].failure, /timeout/i);

  const crossOriginFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "cross-origin",
    suite: "cross-origin.browser.json",
  });
  assert.equal(crossOriginFailure.result.outcome, "failed");
  assert.match(
    crossOriginFailure.result.tests[0].failure,
    /configured origin/i,
  );

  const consoleFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "console-strict",
    policy: { failOnConsoleError: true },
    suite: "console.browser.json",
  });
  assert.equal(consoleFailure.result.outcome, "failed");
  const consoleReport = JSON.parse(
    await fs.readFile(
      path.join(consoleFailure.directory, "console.json"),
      "utf8",
    ),
  );
  assert(consoleReport.entries.some(({ type }) => type === "error"));

  const consoleAllowed = await runSuite({
    baseUrl,
    mode: "portable",
    name: "console-allowed",
    policy: { failOnConsoleError: false },
    suite: "console.browser.json",
  });
  assert.equal(consoleAllowed.result.outcome, "passed");

  const networkFailure = await runSuite({
    baseUrl,
    mode: "studio-run-locally",
    name: "network-strict",
    policy: { failOnNetworkFailure: true },
    runtimePlatform: "windows",
    suite: "network.browser.json",
  });
  assert.equal(networkFailure.result.outcome, "failed");
  const networkReport = JSON.parse(
    await fs.readFile(
      path.join(networkFailure.directory, "network-failures.json"),
      "utf8",
    ),
  );
  assert(networkReport.entries.some(({ status }) => status === 503));
  assert(
    networkReport.entries.every(({ diagnostic }) => diagnostic === undefined),
  );

  for (const [kind, policy, diagnosed, failed] of [
    ["missing", { failOnConsoleError: false }, true, true],
    ["nested", {}, true, true],
    ["missing-assertion", {}, true, true],
    ["missing-console", { failOnNetworkFailure: false }, true, true],
    [
      "missing-allowed",
      { failOnConsoleError: false, failOnNetworkFailure: false },
      true,
      false,
    ],
    ["present", {}, false, false],
    ["unavailable", {}, false, true],
    ["unrelated", {}, false, true],
    ["foreign", {}, false, true],
    ["fetch", {}, false, true],
  ]) {
    const cssRun = await runSuite({
      baseUrl,
      mode: "studio-run-locally",
      name: `widget-css-${kind}`,
      policy,
      suite: {
        schemaVersion: "1.0.0",
        name: "Widget CSS diagnostics",
        tests: [
          {
            name: kind,
            steps: [
              {
                action: "goto",
                path: `/widget-css-fixture?kind=${kind}`,
                waitUntil: "load",
              },
              {
                action: "expectText",
                locator: { by: "role", role: "heading", name: "Loaded" },
                value:
                  kind === "missing-assertion" ? "Missing heading" : "Loaded",
              },
            ],
          },
        ],
      },
    });
    assert.equal(cssRun.result.outcome, failed ? "failed" : "passed", kind);
    const report = JSON.parse(
      await fs.readFile(
        path.join(cssRun.directory, "network-failures.json"),
        "utf8",
      ),
    );
    assert.equal(
      report.entries.some(
        ({ diagnostic }) => diagnostic?.code === "mendix_widget_css_missing",
      ),
      diagnosed,
      kind,
    );
    assert.equal(
      (cssRun.result.tests[0].failure ?? "").includes("Check MPK CSS"),
      diagnosed && failed,
      kind,
    );
    if (diagnosed) {
      assert(
        report.entries.some(
          ({ status, reason }) =>
            status === 404 && reason === "http-error-status",
        ),
      );
      assert.equal(JSON.stringify(report).includes("private-stamp"), false);
      if (failed) {
        const html = await fs.readFile(
          path.join(cssRun.directory, "report.html"),
          "utf8",
        );
        assert(html.includes("widget-css-diagnostics.md"));
      }
    }
    await verifyNoSecret(cssRun.directory);
  }

  await runCompressibleTraceRejection(baseUrl);
  const recovery = await runSuite({
    baseUrl,
    mode: "portable",
    name: "post-limit-recovery",
    suite: "smoke.browser.json",
  });
  assert.equal(recovery.result.outcome, "passed");
  await verifyNoSecret(recovery.directory);

  await verifyBoundedParallelExecution(baseUrl);

  const successSummary = JSON.parse(
    await fs.readFile(path.join(portable.directory, "summary.json"), "utf8"),
  );
  assert.equal(successSummary.schemaVersion, "5.0.0");
  assert.equal(successSummary.tests[0].completedSteps, 10);
  const html = await fs.readFile(
    path.join(portable.directory, "report.html"),
    "utf8",
  );
  assert.match(html, /Mendix fixture smoke/);
  assert.match(html, /Outcome: passed/);

  // A single-worker run keeps the historical report shape: one lane, no
  // per-test deadline, and no overlap to report.
  assert.deepEqual(successSummary.concurrency, {
    requestedWorkers: 1,
    effectiveWorkers: 1,
    limitedBy: "request",
    maxObservedParallel: 1,
    sessionRole: "owner",
    groups: [{ resource: "data-write", mode: "serial", tests: 1 }],
  });

  process.stdout.write(
    "browser E2E: 41 scenarios passed (Portable, WinBoat metadata, env/storage auth, assertion/page/navigation/origin failures, console/network policy, widget CSS diagnostics, missing Chromium, video/HAR, bounded malicious trace, recovery, secret scan, bounded parallel scheduling, sequential/parallel determinism, serial and exclusive groups, parallel failure isolation, worker deadline, concurrency refusals)\n",
  );
} finally {
  server.kill("SIGTERM");
  await Promise.race([onceExit(server), delay(2_000)]);
  if (server.exitCode === null) server.kill("SIGKILL");
  await fs.rm(temporary, { recursive: true, force: true });
}

async function runCompressibleTraceRejection(baseUrl) {
  const directory = path.join(temporary, "compressible-trace");
  await fs.mkdir(directory, { mode: 0o700 });
  const request = {
    schemaVersion: "5.0.0",
    sessionId: `session_${randomBytes(16).toString("hex")}`,
    baseUrl,
    outputDirectory: directory,
    runtimeContext: {
      hostPlatform: "linux",
      studioPlatform: "windows",
      runtimePlatform: "linux",
      backend: "linux-winboat",
      runtimeMode: "portable",
      runtimeVersion: "11.12.2",
    },
    policy: {
      navigationTimeoutMilliseconds: 15_000,
      actionTimeoutMilliseconds: 10_000,
      assertionTimeoutMilliseconds: 10_000,
      failOnConsoleError: true,
      failOnNetworkFailure: true,
      recordVideo: false,
      recordHar: false,
      maxArtifactBytes: 128 * 1024 * 1024,
      retentionRuns: 20,
    },
    suite: JSON.parse(
      await fs.readFile(
        path.join(repository, "tests/browser/compressible-trace.browser.json"),
        "utf8",
      ),
    ),
  };
  const { code, peakResidentBytes, stderr, stdout } = await invokeRunnerRaw(
    "run",
    request,
    {},
    true,
  );
  assert.equal(stderr, "", `browser runner wrote stderr: ${stderr}`);
  assert.equal(stdout.includes(password), false);
  const envelope = JSON.parse(stdout);
  const entries = await fs.readdir(directory);
  const trace = entries.find((name) => name.endsWith("-trace.zip"));
  assert.ok(trace, "malicious response must produce a trace fixture");
  const traceBytes = await fs.readFile(path.join(directory, trace));
  const maximumTraceMemberBytes = Math.max(
    0,
    ...[
      ...inspectZipArchive(traceBytes, {
        ...DEFAULT_ARTIFACT_SAFETY_LIMITS,
        maximumZipCompressionRatio: 1_000_000,
        maximumZipEntryBytes: 1024 * 1024 * 1024,
        maximumZipTotalBytes: 2 * 1024 * 1024 * 1024,
      }).values(),
    ].map(({ uncompressedSize }) => uncompressedSize),
  );
  assert.ok(
    maximumTraceMemberBytes > 64 * 1024 * 1024,
    `trace member ${maximumTraceMemberBytes} must exceed the safety limit`,
  );
  assert.equal(
    code,
    1,
    `ZIP bomb fixture unexpectedly succeeded with maximum member ${maximumTraceMemberBytes}: ${stdout}`,
  );
  assert.equal(envelope.ok, false);
  assert.equal(envelope.error.code, "artifact_limit_exceeded");
  assert.ok(
    peakResidentBytes > 0 &&
      peakResidentBytes < MAX_COMPRESSIBLE_RUNNER_RSS_BYTES,
    `bounded ${maximumTraceMemberBytes}-byte trace runner RSS ${peakResidentBytes} must remain below ${MAX_COMPRESSIBLE_RUNNER_RSS_BYTES}`,
  );
  assert.ok(
    traceBytes.length < 8 * 1024 * 1024,
    "malicious trace must have a small compressed representation",
  );
  assert.equal(entries.includes("artifact-manifest.json"), false);
  assert.equal(
    entries.some((name) => name.endsWith(".redact")),
    false,
  );
  process.stdout.write(
    `browser artifact security: compressed=${traceBytes.length} bytes, maximumMember=${maximumTraceMemberBytes} bytes, runnerPeakRss=${peakResidentBytes} bytes\n`,
  );
}

/// Real-Chromium coverage for the opt-in bounded parallel scheduler (#155):
/// overlap, determinism against the sequential path, the serial and exclusive
/// groups, per-test failure isolation, the worker deadline, the published
/// concurrency report, and every refusal the contract promises.
async function verifyBoundedParallelExecution(baseUrl) {
  const declaredSuite = JSON.parse(
    await fs.readFile(
      path.join(repository, "tests/browser/parallel.browser.json"),
      "utf8",
    ),
  );
  const declaredNames = declaredSuite.tests.map(({ name }) => name);
  const declaredHarNames = declaredNames.map(
    (_, index) => `test-${String(index + 1).padStart(3, "0")}.har`,
  );

  // 1. Several lanes against the fixture server. HAR recording gives every
  // test a distinct artifact of its own, so a shared or overwritten output
  // name would be visible immediately.
  const parallel = await runSuite({
    baseUrl,
    mode: "portable",
    name: "parallel-workers",
    policy: { concurrency: { workers: 4 }, recordHar: true },
    suite: "parallel.browser.json",
  });
  const observed = parallel.result.concurrency;
  assert.equal(parallel.result.outcome, "passed");
  assert.equal(parallel.result.passed, declaredNames.length);
  assert.equal(parallel.result.failed, 0);
  assert.equal(parallel.result.skipped, 0);
  assertConcurrencyReport(observed, 4, "parallel");
  assert.equal(observed.sessionRole, "owner");
  assert.equal(
    observed.testTimeoutMilliseconds,
    DEFAULT_WORKER_TEST_TIMEOUT_MILLISECONDS,
    "more than one worker must adopt the default per-test deadline",
  );
  if (observed.effectiveWorkers > 1) {
    assert.ok(
      observed.maxObservedParallel > 1,
      `a ${observed.effectiveWorkers}-worker run must overlap at least two tests, saw ${observed.maxObservedParallel}`,
    );
  }
  assert.deepEqual(observed.groups, [
    { resource: "app-read", mode: "parallel", tests: 3 },
    // Two tests share one proven scope, so the table counts the scope once.
    { resource: "data-write", mode: "scoped-parallel", tests: 2, scopes: 1 },
    { resource: "data-write", mode: "serial", tests: 1 },
  ]);
  // Completion order is not report order: results stay in declaration order.
  assert.deepEqual(
    parallel.result.tests.map(({ name }) => name),
    declaredNames,
  );
  for (const test of parallel.result.tests) {
    assert.equal(test.outcome, "passed", test.name);
    assert.equal(test.completedSteps, test.totalSteps, test.name);
    assert.equal(test.invalidatedBy, undefined, test.name);
  }
  const parallelNames = artifactNames(parallel.result);
  assert.equal(
    new Set(parallelNames).size,
    parallelNames.length,
    "parallel lanes must not share an artifact name",
  );
  assert.deepEqual(
    parallelNames.filter((name) => name.endsWith(".har")),
    declaredHarNames,
  );
  await verifyManifest(parallel.directory, "portable", "linux");
  await verifyNoSecret(parallel.directory);
  const parallelManifest = JSON.parse(
    await fs.readFile(
      path.join(parallel.directory, "artifact-manifest.json"),
      "utf8",
    ),
  );
  assert.deepEqual(parallelManifest.concurrency, observed);
  assert.equal(parallelManifest.suite.tests, declaredNames.length);
  const parallelHtml = await fs.readFile(
    path.join(parallel.directory, "report.html"),
    "utf8",
  );
  assert.ok(
    parallelHtml.includes(
      `Workers: <strong>${observed.effectiveWorkers}</strong> of 4 requested`,
    ),
    "the HTML report must name the effective worker count",
  );
  assert.match(
    parallelHtml,
    new RegExp(`peak parallel ${observed.maxObservedParallel}`),
  );
  assert.match(parallelHtml, /session role owner/);

  // 2. The same suite, one lane: identical ordered results and artifacts.
  const sequential = await runSuite({
    baseUrl,
    mode: "portable",
    name: "parallel-sequential",
    policy: { concurrency: { workers: 1 }, recordHar: true },
    suite: "parallel.browser.json",
  });
  assert.equal(sequential.result.outcome, "passed");
  assertConcurrencyReport(sequential.result.concurrency, 1, "sequential");
  assert.equal(sequential.result.concurrency.effectiveWorkers, 1);
  assert.equal(sequential.result.concurrency.maxObservedParallel, 1);
  assert.equal(
    sequential.result.concurrency.testTimeoutMilliseconds,
    undefined,
    "one worker must keep the historical unbounded per-test behavior",
  );
  assert.deepEqual(
    sequential.result.concurrency.groups,
    observed.groups,
    "the parallel permission table must not depend on the worker count",
  );
  assert.deepEqual(
    testIdentities(sequential.result),
    testIdentities(parallel.result),
  );
  assert.deepEqual(artifactNames(sequential.result), parallelNames);
  await verifyNoSecret(sequential.directory);

  // The worker limit applies the suite bound last, so a host that could run
  // two lanes at all must attribute a collapse to one lane to the suite. A
  // host already bound to a single lane reports that same host limit instead.
  // Deriving the expectation keeps both cases asserted exactly instead of
  // skipping the check on a small runner. The fixture suite above can always
  // overlap at least four tests, so its own limit is never the suite.
  const collapsedLimit =
    observed.effectiveWorkers > 1 ? "suite" : observed.limitedBy;

  // 3. Nothing in this suite proved its data isolation, so four requested
  // lanes collapse to one and the report says the suite is the limit.
  const serial = await runSuite({
    baseUrl,
    mode: "portable",
    name: "parallel-serial-group",
    policy: { concurrency: { workers: 4 } },
    sessionRole: "participant",
    suite: {
      schemaVersion: "1.0.0",
      name: "Mendix fixture serial data group",
      beforeEach: [{ action: "goto", path: "/" }],
      tests: [
        {
          name: "undeclared legacy write",
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "heading", name: "Fixture sign in" },
            },
          ],
        },
        {
          name: "scoped write without verified isolation",
          concurrency: { resource: "data-write", scope: "fixture-tasks" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "button", name: "Sign in" },
            },
          ],
        },
        {
          name: "second undeclared legacy write",
          steps: [
            {
              action: "expectVisible",
              locator: { by: "testId", value: "cross-origin" },
            },
          ],
        },
      ],
    },
  });
  assert.equal(serial.result.outcome, "passed");
  assertConcurrencyReport(serial.result.concurrency, 4, "serial");
  assert.equal(serial.result.concurrency.effectiveWorkers, 1);
  assert.equal(serial.result.concurrency.maxObservedParallel, 1);
  assert.equal(serial.result.concurrency.limitedBy, collapsedLimit);
  // An attached participant is allowed everything except VM lifecycle work.
  assert.equal(serial.result.concurrency.sessionRole, "participant");
  assert.deepEqual(serial.result.concurrency.groups, [
    { resource: "data-write", mode: "serial", tests: 3 },
  ]);
  await verifyNoSecret(serial.directory);

  // 4. A VM lifecycle test is an exclusive barrier, and the upper bounds of
  // the contract (the highest worker count, the longest deadline) are
  // accepted rather than refused.
  const exclusive = await runSuite({
    baseUrl,
    mode: "portable",
    name: "parallel-exclusive-group",
    policy: {
      concurrency: {
        workers: MAX_BROWSER_WORKERS,
        testTimeoutMilliseconds: MAXIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS,
      },
    },
    suite: {
      schemaVersion: "1.0.0",
      name: "Mendix fixture exclusive group",
      beforeEach: [{ action: "goto", path: "/" }],
      tests: [
        {
          name: "stable app read beside the barrier",
          concurrency: { resource: "app-read" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "heading", name: "Fixture sign in" },
            },
          ],
        },
        {
          name: "owner runs the vm lifecycle turn alone",
          concurrency: { resource: "vm-lifecycle" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "button", name: "Sign in" },
            },
          ],
        },
      ],
    },
  });
  assert.equal(exclusive.result.outcome, "passed");
  assertConcurrencyReport(
    exclusive.result.concurrency,
    MAX_BROWSER_WORKERS,
    "exclusive",
  );
  assert.equal(exclusive.result.concurrency.effectiveWorkers, 1);
  assert.equal(exclusive.result.concurrency.maxObservedParallel, 1);
  assert.equal(exclusive.result.concurrency.limitedBy, collapsedLimit);
  assert.equal(
    exclusive.result.concurrency.testTimeoutMilliseconds,
    MAXIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS,
  );
  assert.equal(exclusive.result.concurrency.sessionRole, "owner");
  assert.deepEqual(exclusive.result.concurrency.groups, [
    { resource: "app-read", mode: "parallel", tests: 1 },
    { resource: "vm-lifecycle", mode: "exclusive", tests: 1 },
  ]);

  // 5. One lane's failure keeps its own evidence and leaves the others alone.
  const isolated = await runSuite({
    baseUrl,
    mode: "portable",
    name: "parallel-failure-isolation",
    policy: { concurrency: { workers: 3 } },
    suite: {
      schemaVersion: "1.0.0",
      name: "Mendix fixture parallel failure isolation",
      beforeEach: [{ action: "goto", path: "/" }],
      tests: [
        {
          name: "first lane passes",
          concurrency: { resource: "app-read" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "heading", name: "Fixture sign in" },
            },
          ],
        },
        {
          name: "second lane fails its assertion",
          concurrency: { resource: "app-read" },
          steps: [
            {
              action: "expectText",
              locator: { by: "role", role: "heading", name: "Fixture sign in" },
              value: "This assertion must fail",
            },
          ],
        },
        {
          name: "third lane passes",
          concurrency: { resource: "app-read" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "button", name: "Sign in" },
            },
          ],
        },
      ],
    },
  });
  assert.equal(isolated.result.outcome, "failed");
  assert.equal(isolated.result.passed, 2);
  assert.equal(isolated.result.failed, 1);
  assert.equal(isolated.result.skipped, 0);
  assertConcurrencyReport(isolated.result.concurrency, 3, "isolated failure");
  assert.deepEqual(
    isolated.result.tests.map(({ outcome }) => outcome),
    ["passed", "failed", "passed"],
  );
  assert.equal(isolated.result.tests[1].invalidatedBy, undefined);
  const isolatedNames = artifactNames(isolated.result);
  for (const suffix of [
    "-failure.png",
    "-dom.html",
    "-accessibility.json",
    "-trace.zip",
  ]) {
    assert.deepEqual(
      isolatedNames.filter((name) => name.endsWith(suffix)),
      [`test-002${suffix}`],
      `only the failed lane may own ${suffix}`,
    );
  }
  await verifyNoSecret(isolated.directory);

  // 6. The per-test deadline invalidates only the lane that overran it.
  const deadline = await runSuite({
    baseUrl,
    mode: "portable",
    name: "parallel-worker-deadline",
    policy: {
      assertionTimeoutMilliseconds: 15_000,
      // The fast lane needs well under a second here, so the deadline keeps a
      // wide margin for a loaded host while still bounding the slow lane.
      concurrency: { workers: 2, testTimeoutMilliseconds: 5_000 },
    },
    suite: {
      schemaVersion: "1.0.0",
      name: "Mendix fixture worker deadline",
      beforeEach: [{ action: "goto", path: "/" }],
      tests: [
        {
          name: "fast lane finishes inside the deadline",
          concurrency: { resource: "app-read" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "role", role: "heading", name: "Fixture sign in" },
            },
          ],
        },
        {
          name: "slow lane waits for an element that never appears",
          concurrency: { resource: "app-read" },
          steps: [
            {
              action: "expectVisible",
              locator: { by: "testId", value: "never-rendered" },
            },
          ],
        },
      ],
    },
  });
  assert.equal(deadline.result.outcome, "failed");
  assert.equal(deadline.result.passed, 1);
  assert.equal(deadline.result.failed, 1);
  assertConcurrencyReport(deadline.result.concurrency, 2, "deadline");
  assert.equal(deadline.result.concurrency.testTimeoutMilliseconds, 5_000);
  assert.equal(deadline.result.tests[0].outcome, "passed");
  assert.equal(deadline.result.tests[0].invalidatedBy, undefined);
  assert.equal(deadline.result.tests[1].outcome, "failed");
  assert.equal(deadline.result.tests[1].invalidatedBy, "timeout");
  assert.match(deadline.result.tests[1].failure, /timeout/i);
  assert.ok(
    artifactNames(deadline.result).includes("test-002-failure.png"),
    "an invalidated lane still keeps its own evidence",
  );
  await verifyNoSecret(deadline.directory);

  // 7. Every refusal the concurrency contract promises, before any browser
  // starts and without writing a single artifact.
  for (const [name, code, overrides] of [
    [
      "refuse-participant-vm-lifecycle",
      "concurrency_policy_refused",
      {
        sessionRole: "participant",
        suite: policyProbeSuite({ resource: "vm-lifecycle" }),
      },
    ],
    [
      "refuse-verified-without-scope",
      "invalid_suite",
      {
        suite: policyProbeSuite({
          resource: "data-write",
          isolation: "verified",
        }),
      },
    ],
    [
      "refuse-verified-app-read",
      "invalid_suite",
      {
        suite: policyProbeSuite({
          resource: "app-read",
          isolation: "verified",
        }),
      },
    ],
    [
      "refuse-unknown-resource",
      "invalid_suite",
      { suite: policyProbeSuite({ resource: "database-write" }) },
    ],
    [
      "refuse-scoped-app-read",
      "invalid_suite",
      {
        suite: policyProbeSuite({
          resource: "app-read",
          scope: "fixture-tasks",
        }),
      },
    ],
    [
      "refuse-invalid-scope",
      "invalid_suite",
      {
        suite: policyProbeSuite({
          resource: "data-write",
          scope: "-fixture tasks",
        }),
      },
    ],
    [
      "refuse-zero-workers",
      "invalid_request",
      { policy: { concurrency: { workers: 0 } } },
    ],
    [
      "refuse-nine-workers",
      "invalid_request",
      { policy: { concurrency: { workers: MAX_BROWSER_WORKERS + 1 } } },
    ],
    [
      "refuse-short-deadline",
      "invalid_request",
      {
        policy: {
          concurrency: {
            workers: 2,
            testTimeoutMilliseconds:
              MINIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS - 1,
          },
        },
      },
    ],
    [
      "refuse-long-deadline",
      "invalid_request",
      {
        policy: {
          concurrency: {
            workers: 2,
            testTimeoutMilliseconds:
              MAXIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS + 1,
          },
        },
      },
    ],
    [
      "refuse-unknown-concurrency-key",
      "invalid_request",
      { policy: { concurrency: { workers: 2, lanes: 2 } } },
    ],
    [
      "refuse-unknown-session-role",
      "invalid_request",
      { sessionRole: "observer" },
    ],
  ]) {
    const { directory, request } = await prepareRun({
      baseUrl,
      mode: "portable",
      name,
      suite: policyProbeSuite({ resource: "app-read" }),
      ...overrides,
    });
    const envelope = await invokeRunnerFailure("run", request);
    assert.equal(envelope.error.code, code, name);
    assert.deepEqual(
      await fs.readdir(directory),
      [],
      `${name} must refuse before writing artifacts`,
    );
  }

  process.stdout.write(
    `browser parallel scheduling: ${observed.effectiveWorkers} of ${observed.requestedWorkers} workers (limited by ${observed.limitedBy}), peak parallel ${observed.maxObservedParallel}, ${declaredNames.length} tests deterministic against the sequential path\n`,
  );
}

function policyProbeSuite(concurrency) {
  return {
    schemaVersion: "1.0.0",
    name: "Mendix fixture concurrency policy probe",
    tests: [
      {
        name: "concurrency policy probe",
        concurrency,
        steps: [{ action: "goto", path: "/" }],
      },
    ],
  };
}

function assertConcurrencyReport(concurrency, requestedWorkers, label) {
  assert.equal(concurrency.requestedWorkers, requestedWorkers, label);
  assert.ok(
    Number.isInteger(concurrency.effectiveWorkers) &&
      concurrency.effectiveWorkers >= 1 &&
      concurrency.effectiveWorkers <= requestedWorkers,
    `${label}: effective workers ${concurrency.effectiveWorkers} must stay within 1..${requestedWorkers}`,
  );
  assert.ok(
    ["request", "cpu", "memory", "suite"].includes(concurrency.limitedBy),
    `${label}: unknown limit ${concurrency.limitedBy}`,
  );
  assert.equal(
    concurrency.limitedBy === "request",
    concurrency.effectiveWorkers === requestedWorkers,
    `${label}: only an unreduced budget may report the request as its limit`,
  );
  assert.ok(
    Number.isInteger(concurrency.maxObservedParallel) &&
      concurrency.maxObservedParallel >= 1 &&
      concurrency.maxObservedParallel <= concurrency.effectiveWorkers,
    `${label}: peak parallelism ${concurrency.maxObservedParallel} must stay within ${concurrency.effectiveWorkers} lanes`,
  );
}

function testIdentities(result) {
  return result.tests.map(({ name, outcome }) => `${name}\u0000${outcome}`);
}

function artifactNames(result) {
  return result.files.map(({ path: name }) => name).sort();
}

async function runSuite({
  baseUrl,
  mode,
  name,
  policy: policyOverrides = {},
  runtimePlatform = "linux",
  sessionRole,
  suite,
}) {
  const { directory, request } = await prepareRun({
    baseUrl,
    mode,
    name,
    policy: policyOverrides,
    runtimePlatform,
    sessionRole,
    suite,
  });
  const result = await invokeRunner("run", request);
  assert.equal(result.sessionId, request.sessionId);
  assert.equal(result.schemaVersion, "5.0.0");
  // These suites never pass an asset mirror URL, so the runner must report an
  // unmodified browser with an explicit, unused correction record (#141).
  assert.equal(result.browserParity, "unmodified");
  assert.deepEqual(result.corrections, [
    { kind: "host-lan-asset-mirror", applied: false, interceptedRequests: 0 },
  ]);
  const actualFiles = new Set(await fs.readdir(directory));
  assert.deepEqual(
    actualFiles,
    new Set(result.files.map(({ path: filename }) => filename)),
    "runner output and artifact inventory must match exactly",
  );
  return { directory, result };
}

/// Build one runner request and its empty output directory. Refusal scenarios
/// reuse it so an invalid request is shaped exactly like a valid one.
async function prepareRun({
  baseUrl,
  mode,
  name,
  policy: policyOverrides = {},
  runtimePlatform = "linux",
  sessionRole,
  suite,
}) {
  const directory = path.join(temporary, name);
  await fs.mkdir(directory, { mode: 0o700 });
  const sessionId = `session_${randomBytes(16).toString("hex")}`;
  const policy = {
    navigationTimeoutMilliseconds: 15_000,
    actionTimeoutMilliseconds: 5_000,
    assertionTimeoutMilliseconds: 1_000,
    failOnConsoleError: true,
    failOnNetworkFailure: true,
    recordVideo: false,
    recordHar: false,
    maxArtifactBytes: 128 * 1024 * 1024,
    retentionRuns: 20,
    ...policyOverrides,
  };
  const request = {
    schemaVersion: "5.0.0",
    sessionId,
    baseUrl,
    outputDirectory: directory,
    runtimeContext: {
      hostPlatform: "linux",
      studioPlatform: "windows",
      runtimePlatform,
      backend: "linux-winboat",
      runtimeMode: mode,
      ...(mode === "studio-run-locally" ? { studioVersion: "11.12.2" } : {}),
      runtimeVersion: "11.12.2",
    },
    policy,
    ...(sessionRole === undefined ? {} : { sessionRole }),
    suite:
      typeof suite === "string"
        ? JSON.parse(
            await fs.readFile(
              path.join(repository, "tests/browser", suite),
              "utf8",
            ),
          )
        : suite,
  };
  return { directory, request };
}

async function invokeRunner(command, request, environment = {}) {
  const { code, stderr, stdout } = await invokeRunnerRaw(
    command,
    request,
    environment,
  );
  assert.equal(stderr, "", `browser runner wrote stderr: ${stderr}`);
  assert.equal(stdout.trim().split("\n").length, 1);
  const envelope = JSON.parse(stdout);
  assert.equal(code, 0, `browser runner failed: ${JSON.stringify(envelope)}`);
  assert.equal(envelope.ok, true);
  return envelope.data;
}

async function invokeRunnerFailure(command, request, environment = {}) {
  const { code, stderr, stdout } = await invokeRunnerRaw(
    command,
    request,
    environment,
  );
  assert.equal(stderr, "", `browser runner wrote stderr: ${stderr}`);
  assert.equal(stdout.trim().split("\n").length, 1);
  const envelope = JSON.parse(stdout);
  assert.equal(code, 1, `browser runner unexpectedly succeeded: ${stdout}`);
  assert.equal(envelope.ok, false);
  return envelope;
}

async function invokeRunnerRaw(
  command,
  request,
  environment,
  measureMemory = false,
) {
  const child = spawn(process.execPath, [runner, command], {
    cwd: repository,
    env: {
      ...process.env,
      MENDIMARU_TEST_PASSWORD: password,
      MENDIMARU_TEST_STORAGE_STATE: storageStatePath,
      MENDIMARU_TEST_USERNAME: username,
      ...environment,
    },
    stdio: [request ? "pipe" : "ignore", "pipe", "pipe"],
  });
  if (request) child.stdin.end(JSON.stringify(request));
  const [stdout, stderr, code, peakResidentBytes] = await Promise.all([
    collect(child.stdout),
    collect(child.stderr),
    onceExit(child),
    measureMemory ? peakLinuxResidentBytes(child) : Promise.resolve(null),
  ]);
  return { code, peakResidentBytes, stderr, stdout };
}

async function peakLinuxResidentBytes(child) {
  if (process.platform !== "linux") return 0;
  let peak = 0;
  while (child.exitCode === null) {
    const status = await fs
      .readFile(`/proc/${child.pid}/status`, "utf8")
      .catch(() => "");
    const match = /^VmRSS:\s+(\d+)\s+kB$/m.exec(status);
    if (match) peak = Math.max(peak, Number(match[1]) * 1024);
    if (child.exitCode === null) await delay(10);
  }
  return peak;
}

async function verifyManifest(directory, mode, runtimePlatform) {
  const manifest = JSON.parse(
    await fs.readFile(path.join(directory, "artifact-manifest.json"), "utf8"),
  );
  assert.equal(manifest.schemaVersion, "5.0.0");
  assert.deepEqual(manifest.corrections, [
    { kind: "host-lan-asset-mirror", applied: false, interceptedRequests: 0 },
  ]);
  assert.equal(manifest.hostPlatform, "linux");
  assert.equal(manifest.studioPlatform, "windows");
  assert.equal(manifest.runtimePlatform, runtimePlatform);
  assert.equal(manifest.backend, "linux-winboat");
  assert.equal(manifest.runtimeMode, mode);
  assert.equal(manifest.runtimeVersion, "11.12.2");
  if (mode === "studio-run-locally") {
    assert.equal(manifest.studioVersion, "11.12.2");
  }
  assert.equal(manifest.browser.name, "chromium");
  assert.match(manifest.browser.version, /^\d+/);
  assert.match(manifest.playwrightVersion, /^\d+\.\d+\.\d+$/);
  assert(manifest.artifacts.length >= 5);
  for (const artifact of manifest.artifacts) {
    const bytes = await fs.readFile(path.join(directory, artifact.file));
    assert.equal(artifact.sizeBytes, bytes.length);
    assert.equal(
      artifact.sha256,
      createHash("sha256").update(bytes).digest("hex"),
    );
  }
}

async function verifyNoSecret(directory) {
  const needles = [
    Buffer.from(password),
    Buffer.from(encodeURIComponent(password)),
    Buffer.from(encodeURIComponent(password).replaceAll("%20", "+")),
    Buffer.from(
      encodeURIComponent(password).replace(/%[0-9A-F]{2}/g, (value) =>
        value.toLowerCase(),
      ),
    ),
    Buffer.from(
      encodeURIComponent(password)
        .replace(/%[0-9A-F]{2}/g, (value) => value.toLowerCase())
        .replaceAll("%20", "+"),
    ),
    Buffer.from(JSON.stringify(password).slice(1, -1)),
    Buffer.from(htmlEscape(password, false)),
    Buffer.from(htmlEscape(password, true)),
    Buffer.from(Buffer.from(password).toString("base64")),
    Buffer.from(
      Buffer.from(password)
        .toString("base64")
        .replaceAll("+", "-")
        .replaceAll("/", "_"),
    ),
    Buffer.from(Buffer.from(password).toString("base64url")),
  ];
  for (const name of await fs.readdir(directory)) {
    const bytes = await fs.readFile(path.join(directory, name));
    const archive = name.endsWith(".zip") ? unzipSync(bytes) : null;
    const payloads = archive ? Object.values(archive) : [bytes];
    if (archive) {
      for (const memberName of Object.keys(archive)) {
        for (const needle of needles) {
          assert.equal(
            Buffer.from(memberName).includes(needle),
            false,
            `secret leaked into ${name} member name`,
          );
        }
      }
    }
    for (const payload of payloads) {
      for (const needle of needles) {
        assert.equal(
          Buffer.from(payload).includes(needle),
          false,
          `secret leaked into ${name}`,
        );
      }
    }
  }
}

function htmlEscape(value, quote) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', quote ? "&quot;" : '"')
    .replaceAll("'", quote ? "&#39;" : "'");
}

async function firstJsonLine(stream) {
  const lines = readline.createInterface({ input: stream });
  for await (const line of lines) {
    lines.close();
    return JSON.parse(line);
  }
  throw new Error("fixture server exited before publishing its port");
}

async function collect(stream) {
  let value = "";
  stream.setEncoding("utf8");
  for await (const chunk of stream) value += chunk;
  return value;
}

function onceExit(child) {
  if (child.exitCode !== null) return Promise.resolve(child.exitCode);
  return new Promise((resolve) =>
    child.once("exit", (code) => resolve(code ?? 1)),
  );
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
