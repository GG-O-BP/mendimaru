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
import { createWebviewDriver } from "./webview-driver.mjs";

const repository = path.resolve(import.meta.dirname, "..", "..");
const platform = process.platform === "win32" ? "windows" : process.platform;
if (!["linux", "windows"].includes(platform)) {
  throw new Error("release WebView performance runs only on Linux and Windows");
}
const options = parseArguments(process.argv.slice(2));
const sampling = samplingPolicy();
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
  driver = await createWebviewDriver({
    application: options.application,
    env,
    root: fixture.root,
  });

  await fixture.clearWebviewCache();
  await driver.launch();
  await driver.firstIpc();
  await driver.stop();
  assertions.push("one cold launch and first IPC warm-up were excluded");

  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
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

  await driver.launch();
  const webviewVersion = driver.webviewVersion();
  const client = driver.client;
  assert.ok(client, "release WebDriver client is unavailable");

  await fixture.setEnvironmentMode("slow");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
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

  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    await fixture.setEnvironmentMode("timeout");
    const recoveryStarted = performance.now();
    await assert.rejects(
      client.invoke(
        "get_environment_status",
        {},
        sampling.environmentClientTimeoutMs,
      ),
      (error) => error?.name === "TimeoutError",
      "the delayed environment probe must exceed the client deadline",
    );
    await fixture.setEnvironmentMode("normal");
    const recovered = await client.invoke("get_environment_status");
    assert.equal(recovered.ready, true);
    environmentTimeoutRecoveryMs.push(
      rounded(performance.now() - recoveryStarted),
    );
  }
  assertions.push(
    `${sampling.sampleCount} client timeouts recovered through the next environment probe`,
  );

  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
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

  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
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

  await fixture.setWorkspace(fixture.smallWorkspace);
  await client.invoke("get_projects");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    smallWorkspaceScanMs.push(
      await elapsed(async () => {
        const projects = await client.invoke("get_projects");
        assert.equal(
          projects.projects.length,
          fixture.workspaceTiers.small.projectCount,
        );
      }),
    );
  }
  await fixture.setWorkspace(fixture.largeWorkspace);
  await client.invoke("get_projects");
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
    largeWorkspaceScanMs.push(
      await elapsed(async () => {
        const projects = await client.invoke("get_projects");
        assert.equal(
          projects.projects.length,
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
  for (let sample = 0; sample < sampling.sampleCount; sample += 1) {
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

  await delay(sampling.idleSampleSeconds * 1000);
  if (driver.applicationPid) {
    process.stdout.write(
      `release performance root PID ${driver.applicationPid}\n`,
    );
  }
  const before = await driver.snapshot();
  let previous = before;
  let peak = { ...before };
  const idleSamples = Math.ceil(
    sampling.idleWindowSeconds / sampling.idleSampleSeconds,
  );
  for (let sample = 0; sample < idleSamples; sample += 1) {
    const windowStarted = performance.now();
    await delay(sampling.idleSampleSeconds * 1000);
    const current = await driver.snapshot();
    const elapsedSeconds = (performance.now() - windowStarted) / 1000;
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
} catch (error) {
  await writeFile(
    failurePath,
    `${JSON.stringify(
      {
        status: "failed",
        startedAt,
        finishedAt: new Date().toISOString(),
        application: options.application,
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
    report: values.report,
    commit,
    baselineCommit,
    packageKind,
    runId: values["run-id"] ?? process.env.GITHUB_RUN_ID ?? "local",
  };
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
