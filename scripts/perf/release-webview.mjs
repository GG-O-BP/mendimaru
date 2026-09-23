import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

import {
  commonHostMetadata,
  createPerformanceReport,
  normalizedCpuPercent,
  resourceSummary,
  samplingPolicy,
} from "./performance-core.mjs";
import { createReleaseFixture } from "./release-fixture.mjs";
import {
  createWebviewDriver,
  scriptRequestTimeoutMs,
  SCRIPT_TIMEOUT_MS,
} from "./webview-driver.mjs";

const repository = path.resolve(import.meta.dirname, "..", "..");
const platform = process.platform === "win32" ? "windows" : process.platform;
if (!["linux", "windows"].includes(platform)) {
  throw new Error("release WebView performance runs only on Linux and Windows");
}
const options = parseArguments(process.argv.slice(2));
const sampling = samplingPolicy({
  sampleCount: options.sampleCount,
  idleWindowSeconds: options.idleWindowSeconds,
});
const reportPath = path.resolve(
  options.report ??
    path.join(
      repository,
      "artifacts",
      "e2e",
      `${platform}-release-performance.json`,
    ),
);
const screenshotPath = reportPath.replace(/\.json$/i, ".png");
const failurePath = reportPath.replace(/\.json$/i, ".failure.json");
const startedAt = new Date().toISOString();
const processStarted = performance.now();
// Issue #191. A WebDriver stall used to surface only as a stack frame, so the
// failing run could not be attributed to a measurement phase from the logs.
// Every phase now announces itself and the timeline is persisted with the
// failure so the stall point is identifiable without re-running anything.
const stageTimeline = [];
let activeStage;
const coldStartupMs = [];
const warmStartupMs = [];
const firstIpcMs = [];
const environmentSlowMs = [];
const environmentTimeoutRecoveryMs = [];
const catalogCachedMs = [];
const catalogRefreshMs = [];
const smallWorkspaceScanMs = [];
const largeWorkspaceScanMs = [];
const navigationMs = [];
const idleCpuPercent = [];
const privateMemoryBytes = [];
const workingSetBytes = [];
const processCount = [];
const assertions = [];
let fixture;
let driver;

try {
  beginStage("fixture-setup");
  await mkdir(path.dirname(reportPath), { recursive: true });
  fixture = await createReleaseFixture(process.platform);
  const env = {
    ...process.env,
    MENDIMARU_E2E_ROOT: fixture.root,
    MENDIMARU_E2E_MARKETPLACE_URL: fixture.marketplaceUrl,
    PATH: `${fixture.bin}${path.delimiter}${process.env.PATH ?? ""}`,
    XDG_CACHE_HOME: fixture.webviewCache,
    WEBVIEW2_USER_DATA_FOLDER: fixture.webviewData,
    APPIMAGE_EXTRACT_AND_RUN: "1",
    WEBKIT_DISABLE_DMABUF_RENDERER:
      process.env.WEBKIT_DISABLE_DMABUF_RENDERER ?? "1",
  };
  beginStage("driver-create");
  driver = await createWebviewDriver({
    application: options.application,
    env,
    root: fixture.root,
  });

  beginStage("warmup-launch");
  await fixture.clearWebviewCache();
  await driver.launch();
  await driver.firstIpc();
  await driver.stop();
  assertions.push("one cold launch and first IPC warm-up were excluded");

  beginStage("startup-samples");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    await fixture.clearWebviewCache();
    coldStartupMs.push(await driver.launch());
    firstIpcMs.push(await driver.firstIpc());
    await driver.stop();

    warmStartupMs.push(await driver.launch());
    await driver.stop();
  }
  assertions.push(
    `${sampling.sampleCount} cold and warm release process launches completed`,
  );

  beginStage("measurement-session-open");
  await driver.launch();
  const webviewVersion = driver.webviewVersion();
  const client = driver.client;
  assert.ok(client, "release WebDriver client is unavailable");
  const declaredScriptTimeoutMs = client.scriptTimeoutMs;
  assertions.push(
    declaredScriptTimeoutMs === undefined
      ? "the WebDriver session inherited an undeclared script deadline"
      : `the WebDriver session declared a ${declaredScriptTimeoutMs} ms script deadline`,
  );

  beginStage("environment-slow-samples");
  await fixture.setEnvironmentMode("slow");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    environmentSlowMs.push(
      await elapsed(async () => {
        const environment = await client.invoke("get_environment_status");
        assert.equal(environment.ready, true);
      }),
    );
  }
  await fixture.setEnvironmentMode("normal");
  assertions.push(
    `${sampling.sampleCount} environment probes included the tracked ${sampling.environmentSlowDelayMs} ms slow-backend delay`,
  );

  beginStage("environment-timeout-recovery-samples");
  // Issue #191. The deliberate deadline used to be a client-side fetch abort
  // while the server script deadline stayed at its inherited 30 s. WebDriver
  // has no command-cancel, so the abandoned script kept running and the next
  // command queued behind it; when the orphan outlived its injected delay the
  // following command hit the server deadline and the whole measurement died
  // with an unexplained "script timed out after 30000ms". The deadline is now
  // declared on the server for the probe itself, so the probe ends the script
  // instead of abandoning it and the recovery probe starts from a clean queue.
  const serverEnforcedProbe =
    declaredScriptTimeoutMs !== undefined &&
    (await client.declareScriptTimeout(sampling.environmentClientTimeoutMs));
  assertions.push(
    serverEnforcedProbe
      ? "the deliberate environment deadline was enforced by the WebDriver script timeout, leaving no orphaned script"
      : "the deliberate environment deadline fell back to a client request abort because the script timeout could not be declared",
  );
  try {
    for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
      trackSample(sample, sampling.sampleCount);
      await fixture.setEnvironmentMode("timeout");
      const recoveryStarted = performance.now();
      await assert.rejects(
        serverEnforcedProbe
          ? client.invoke(
              "get_environment_status",
              {},
              scriptRequestTimeoutMs(sampling.environmentClientTimeoutMs),
            )
          : client.invoke(
              "get_environment_status",
              {},
              sampling.environmentClientTimeoutMs,
            ),
        (error) =>
          serverEnforcedProbe
            ? error?.name === "ScriptTimeoutError"
            : error?.name === "TimeoutError",
        "the delayed environment probe must exceed the declared deadline",
      );
      await fixture.setEnvironmentMode("normal");
      const recovered = await client.invoke("get_environment_status");
      assert.equal(recovered.ready, true);
      environmentTimeoutRecoveryMs.push(
        rounded(performance.now() - recoveryStarted),
      );
    }
  } finally {
    if (serverEnforcedProbe) {
      await client
        .declareScriptTimeout(SCRIPT_TIMEOUT_MS)
        .catch(() => undefined);
    }
    await fixture.setEnvironmentMode("normal").catch(() => undefined);
  }
  assertions.push(
    `${sampling.sampleCount} client timeouts recovered through the next environment probe`,
  );

  beginStage("catalog-cached-samples");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    catalogCachedMs.push(
      await elapsed(async () => {
        const catalog = await client.invoke("get_downloadable_versions_cache");
        assert.ok(catalog.versions.length > 0, "cached catalog is empty");
      }),
    );
  }
  assertions.push(
    "cached catalog reads stayed separate from browser refreshes",
  );

  beginStage("catalog-refresh-samples");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    catalogRefreshMs.push(
      await elapsed(async () => {
        const catalog = await client.invoke("fetch_downloadable_versions", {
          page: 1,
          reset: true,
        });
        assert.equal(catalog.versions.length, 2);
        assert.ok(catalog.loadedPages.includes(1));
      }),
    );
  }
  assertions.push(
    "isolated loopback Marketplace refresh used a real sandboxed browser",
  );

  beginStage("small-workspace-scan-samples");
  await fixture.setWorkspace(fixture.smallWorkspace);
  await scanProjectsAfterWorkspaceChange(client);
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    smallWorkspaceScanMs.push(
      await elapsed(async () => {
        const projects = await scanProjectsAfterWorkspaceChange(client);
        assert.equal(
          projectScanCount(projects),
          fixture.workspaceTiers.small.projectCount,
        );
      }),
    );
  }
  beginStage("large-workspace-scan-samples");
  await fixture.setWorkspace(fixture.largeWorkspace);
  await scanProjectsAfterWorkspaceChange(client);
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    largeWorkspaceScanMs.push(
      await elapsed(async () => {
        const projects = await scanProjectsAfterWorkspaceChange(client);
        assert.equal(
          projectScanCount(projects),
          fixture.workspaceTiers.large.projectCount,
        );
      }),
    );
  }
  await fixture.setWorkspace(fixture.smallWorkspace);
  assertions.push(
    `workspace tiers scanned ${fixture.workspaceTiers.small.projectCount} and ${fixture.workspaceTiers.large.projectCount} projects`,
  );

  const routes = [
    ["[data-testid=nav-projects]", "Projects"],
    ["[data-testid=nav-settings]", "Settings"],
    ["[data-testid=nav-studio]", "Studio Pro"],
  ];
  beginStage("navigation-samples");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    trackSample(sample, sampling.sampleCount);
    const [selector, heading] = routes[sample % routes.length];
    navigationMs.push(
      await elapsed(async () => {
        await client.click(selector);
        await waitFor(
          async () =>
            (await client.executeSync(
              "return document.querySelector('main h1')?.textContent || '';",
            )) === heading,
          5_000,
          `${heading} navigation`,
        );
      }),
    );
  }
  assertions.push(
    "release WebView route navigation completed through WebDriver",
  );

  beginStage("idle-settle");
  await delay(sampling.idleSampleSeconds * 1000);
  if (driver.applicationPid) {
    process.stdout.write(
      `release performance root PID ${driver.applicationPid}\n`,
    );
  }
  beginStage("idle-sampling");
  const before = await driver.snapshot();
  let previous = before;
  let peak = { ...before };
  const idleSamplingStarted = performance.now();
  let previousSampleFinished = idleSamplingStarted;
  const idleSampleMilliseconds = sampling.idleSampleSeconds * 1000;
  const idleSamples = Math.ceil(
    sampling.idleWindowSeconds / sampling.idleSampleSeconds,
  );
  for (let sample = 0; sample < idleSamples; sample += 1) {
    trackSample(sample, idleSamples);
    await delayUntil(
      idleSamplingStarted + (sample + 1) * idleSampleMilliseconds,
    );
    const current = await driver.snapshot();
    const sampleFinished = performance.now();
    const elapsedSeconds = (sampleFinished - previousSampleFinished) / 1000;
    idleCpuPercent.push(
      normalizedCpuPercent({
        beforeCpuSeconds: previous.cpuSeconds,
        afterCpuSeconds: current.cpuSeconds,
        elapsedSeconds,
        logicalCores: os.cpus().length,
      }),
    );
    privateMemoryBytes.push(current.privateMemoryBytes);
    workingSetBytes.push(current.workingSetBytes);
    processCount.push(current.processCount);
    peak = maximumSnapshot(peak, current);
    previous = current;
    previousSampleFinished = sampleFinished;
    if ((sample + 1) % 12 === 0) {
      process.stdout.write(
        `release performance idle sample ${sample + 1}/${idleSamples}\n`,
      );
    }
  }
  const after = previous;
  const resources = resourceSummary(before, after, peak);
  assertions.push(
    `process-tree CPU and memory were sampled for ${sampling.idleWindowSeconds} seconds`,
  );

  await driver.screenshot(screenshotPath);
  beginStage("report-write");
  const report = createPerformanceReport({
    benchmark: {
      suite: "release-webview",
      platform,
      buildProfile: "release",
      packageKind: options.packageKind,
      commit: options.commit,
      baselineCommit: options.baselineCommit,
      startedAt,
      finishedAt: new Date().toISOString(),
      runId: options.runId,
    },
    host: commonHostMetadata({
      os: platform,
      osVersion: driver.osVersion(),
      arch: process.arch,
      runnerImage: runnerImage(),
      webviewVersion,
    }),
    fixture: {
      workspaceTiers: fixture.workspaceTiers,
      catalogModes: ["cached", "isolated-refresh"],
      environmentModes: ["normal", "slow", "timeout-recovery"],
    },
    sampling,
    metricSamples: {
      coldStartupMs: { unit: "ms", samples: coldStartupMs },
      warmStartupMs: { unit: "ms", samples: warmStartupMs },
      firstIpcMs: { unit: "ms", samples: firstIpcMs },
      environmentSlowMs: { unit: "ms", samples: environmentSlowMs },
      environmentTimeoutRecoveryMs: {
        unit: "ms",
        samples: environmentTimeoutRecoveryMs,
      },
      catalogCachedMs: { unit: "ms", samples: catalogCachedMs },
      catalogRefreshMs: { unit: "ms", samples: catalogRefreshMs },
      smallWorkspaceScanMs: { unit: "ms", samples: smallWorkspaceScanMs },
      largeWorkspaceScanMs: { unit: "ms", samples: largeWorkspaceScanMs },
      navigationMs: { unit: "ms", samples: navigationMs },
      backgroundPollingCpuPercent: {
        unit: "percent",
        samples: idleCpuPercent.slice(0, 12),
      },
      idleCpuPercent: { unit: "percent", samples: idleCpuPercent },
      privateMemoryBytes: { unit: "bytes", samples: privateMemoryBytes },
      workingSetBytes: { unit: "bytes", samples: workingSetBytes },
      processCount: { unit: "count", samples: processCount },
      privateMemoryGrowthBytes: {
        unit: "bytes",
        samples: [Math.max(0, resources.delta.privateMemoryBytes)],
      },
      workingSetGrowthBytes: {
        unit: "bytes",
        samples: [Math.max(0, resources.delta.workingSetBytes)],
      },
      processCountGrowth: {
        unit: "count",
        samples: [Math.max(0, resources.delta.processCount)],
      },
    },
    resources,
    assertions,
  });
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`release performance report: ${reportPath}\n`);
  closeActiveStage();
} catch (error) {
  closeActiveStage("failed");
  const diagnosis = classifyFailure(error);
  // Issue #191. A failure here is not a budget verdict: the gate never ran.
  // The classification, the failing stage and the failing WebDriver command
  // are written to stdout and to the failure report so a harness stall can be
  // told apart from a measured regression at check level, without re-running
  // the job and without weakening `preserve-original-failure`.
  process.stdout.write(
    `release performance failed: classification=${diagnosis.classification} reason=${diagnosis.reason} stage=${activeStage?.name ?? "none"} sample=${activeStage?.sample ?? "n/a"}\n`,
  );
  const failingCommand = error?.webdriverCommand;
  if (failingCommand) {
    process.stdout.write(
      `release performance failing WebDriver command: ${failingCommand.method} ${failingCommand.endpoint} elapsed=${failingCommand.elapsedMs}ms requestDeadline=${failingCommand.requestTimeoutMs}ms scriptDeadline=${failingCommand.scriptTimeoutMs ?? "inherited"}\n`,
    );
  }
  await writeFile(
    failurePath,
    `${JSON.stringify(
      {
        status: "failed",
        classification: diagnosis.classification,
        reason: diagnosis.reason,
        startedAt,
        finishedAt: new Date().toISOString(),
        application: options.application,
        platform,
        stage: activeStage?.name,
        stageSample: activeStage?.sample,
        stages: stageTimeline,
        webdriverCommand: failingCommand,
        collectedSampleCounts: collectedSampleCounts(),
        error: error instanceof Error ? error.stack : String(error),
      },
      null,
      2,
    )}\n`,
  ).catch(() => undefined);
  throw error;
} finally {
  await driver?.close().catch(() => undefined);
  await fixture?.close().catch(() => undefined);
}

function beginStage(name) {
  closeActiveStage();
  activeStage = {
    name,
    startedAtMs: rounded(performance.now() - processStarted),
    elapsedMs: undefined,
    sample: undefined,
    status: "running",
  };
  stageTimeline.push(activeStage);
  process.stdout.write(
    `release performance stage start: ${name} (t+${(activeStage.startedAtMs / 1000).toFixed(1)}s)\n`,
  );
}

function closeActiveStage(status = "completed") {
  if (!activeStage || activeStage.status !== "running") return;
  activeStage.elapsedMs = rounded(
    performance.now() - processStarted - activeStage.startedAtMs,
  );
  activeStage.status = status;
}

function trackSample(index, total) {
  if (activeStage) activeStage.sample = `${index + 1}/${total}`;
}

// Harness failures and measured-contract failures reach the same catch block,
// so the distinction has to be made from the error itself. WebDriver transport
// and deadline errors carry the command that produced them; assertions carry
// Node's ERR_ASSERTION code.
function classifyFailure(error) {
  if (error?.name === "ScriptTimeoutError") {
    return { classification: "harness", reason: "webdriver-script-timeout" };
  }
  if (error?.name === "TimeoutError") {
    return { classification: "harness", reason: "webdriver-request-deadline" };
  }
  if (error?.name === "WebDriverError" || error?.webdriverCommand) {
    return { classification: "harness", reason: "webdriver-command-failed" };
  }
  if (error?.code === "ERR_ASSERTION") {
    return { classification: "measurement", reason: "assertion-failed" };
  }
  return { classification: "unknown", reason: error?.name ?? "error" };
}

function collectedSampleCounts() {
  return Object.fromEntries(
    Object.entries({
      coldStartupMs,
      warmStartupMs,
      firstIpcMs,
      environmentSlowMs,
      environmentTimeoutRecoveryMs,
      catalogCachedMs,
      catalogRefreshMs,
      smallWorkspaceScanMs,
      largeWorkspaceScanMs,
      navigationMs,
      idleCpuPercent,
    }).map(([name, samples]) => [name, samples.length]),
  );
}

function parseArguments(arguments_) {
  const values = {};
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      throw new Error(`invalid release performance argument: ${name ?? ""}`);
    }
    values[name.slice(2)] = value;
  }
  const application = values.application;
  if (!application) throw new Error("--application is required");
  const commit = values.commit ?? gitCommit();
  const baselineCommit = values["baseline-commit"] ?? commit;
  for (const [name, value] of Object.entries({ commit, baselineCommit })) {
    if (!/^[0-9a-f]{40}$/.test(value)) {
      throw new Error(`--${name} must be a full lowercase Git commit`);
    }
  }
  const packageKind = values["package-kind"] ?? "release-executable";
  if (!["release-executable", "appimage"].includes(packageKind)) {
    throw new Error(
      "release WebView package kind must be release-executable or appimage",
    );
  }
  return {
    application: path.resolve(application),
    sampleCount: positiveIntegerOption(
      values["sample-count"],
      7,
      "--sample-count",
    ),
    idleWindowSeconds: positiveIntegerOption(
      values["idle-window-seconds"],
      300,
      "--idle-window-seconds",
    ),
    report: values.report,
    commit,
    baselineCommit,
    packageKind,
    runId: values["run-id"] ?? process.env.GITHUB_RUN_ID ?? "local",
  };
}

function positiveIntegerOption(value, fallback, name) {
  if (value === undefined) return fallback;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive integer`);
  }
  return parsed;
}

function gitCommit() {
  return execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: repository,
    encoding: "utf8",
  }).trim();
}

async function elapsed(action) {
  const started = performance.now();
  await action();
  return rounded(performance.now() - started);
}

function projectScanCount(projects) {
  if (Array.isArray(projects)) return projects.length;
  return projects.projects?.length;
}

async function scanProjectsAfterWorkspaceChange(client) {
  let lastError;
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      return await client.invoke("get_projects");
    } catch (error) {
      lastError = error;
      if (!error?.message?.includes("workspace scan superseded")) throw error;
      await delay(100);
    }
  }
  throw lastError;
}

async function waitFor(action, timeoutMs, label) {
  const deadline = performance.now() + timeoutMs;
  let lastError;
  while (performance.now() < deadline) {
    try {
      if (await action()) return;
    } catch (error) {
      lastError = error;
    }
    await delay(50);
  }
  throw new Error(
    `timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`,
  );
}

async function delayUntil(deadline) {
  while (performance.now() < deadline) {
    await delay(Math.max(1, Math.ceil(deadline - performance.now())));
  }
}

function maximumSnapshot(left, right) {
  return {
    processCount: Math.max(left.processCount, right.processCount),
    privateMemoryBytes: Math.max(
      left.privateMemoryBytes,
      right.privateMemoryBytes,
    ),
    workingSetBytes: Math.max(left.workingSetBytes, right.workingSetBytes),
    cpuSeconds: Math.max(left.cpuSeconds, right.cpuSeconds),
  };
}

function runnerImage() {
  const image = process.env.ImageOS ?? process.env.RUNNER_OS ?? "local";
  const version = process.env.ImageVersion ?? os.release();
  return `${image}-${version}`;
}

function rounded(value) {
  return Math.round(value * 1000) / 1000;
}
