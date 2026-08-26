import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";

import {
  commonHostMetadata,
  createPerformanceReport,
  resourceSummary,
  samplingPolicy,
} from "./performance-core.mjs";

if (process.platform !== "win32") {
  throw new Error("installed bundle reports are produced on Windows only");
}
const options = parseArguments(process.argv.slice(2));
const raw = JSON.parse(await readFile(options.raw, "utf8"));
if (raw.schemaVersion !== "raw-1.0.0" || raw.status !== "passed") {
  throw new Error(
    `bundle smoke did not produce a passing raw report: ${raw.status}`,
  );
}
const expectedSampling = samplingPolicy();
for (const name of [
  "warmupCount",
  "sampleCount",
  "idleWindowSeconds",
  "idleSampleSeconds",
]) {
  if (raw.sampling[name] !== expectedSampling[name]) {
    throw new Error(
      `bundle smoke ${name} ${raw.sampling[name]} does not match tracked policy ${expectedSampling[name]}`,
    );
  }
}
const before = normalizeSnapshot(raw.resources.before);
const after = normalizeSnapshot(raw.resources.after);
const peak = normalizeSnapshot(raw.resources.peak);
const resources = resourceSummary(before, after, peak);
for (const name of [
  "coldStartupMs",
  "warmStartupMs",
  "idleCpuPercent",
  "privateMemoryBytes",
  "workingSetBytes",
  "processCount",
]) {
  if (!Array.isArray(raw.measurements[name])) {
    throw new Error(`bundle smoke is missing ${name} samples`);
  }
}
const report = createPerformanceReport({
  benchmark: {
    suite: "installed-bundle",
    platform: "windows",
    buildProfile: "release",
    packageKind: raw.installerKind,
    commit: options.commit,
    baselineCommit: options.baselineCommit,
    startedAt: new Date(raw.startedAt).toISOString(),
    finishedAt: new Date(raw.finishedAt).toISOString(),
    runId: options.runId,
  },
  host: commonHostMetadata({
    os: "windows",
    osVersion: powershell(
      "[Environment]::OSVersion.VersionString + ' ' + (Get-CimInstance Win32_OperatingSystem).Caption",
    ),
    arch: process.arch,
    runnerImage: runnerImage(),
    webviewVersion: raw.webviewVersion,
  }),
  fixture: { workspaceTiers: null, catalogModes: [], environmentModes: [] },
  sampling: expectedSampling,
  metricSamples: {
    coldStartupMs: { unit: "ms", samples: raw.measurements.coldStartupMs },
    warmStartupMs: { unit: "ms", samples: raw.measurements.warmStartupMs },
    idleCpuPercent: {
      unit: "percent",
      samples: raw.measurements.idleCpuPercent,
    },
    privateMemoryBytes: {
      unit: "bytes",
      samples: raw.measurements.privateMemoryBytes,
    },
    workingSetBytes: {
      unit: "bytes",
      samples: raw.measurements.workingSetBytes,
    },
    processCount: { unit: "count", samples: raw.measurements.processCount },
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
    installMs: { unit: "ms", samples: [raw.measurements.installMs] },
    uninstallMs: { unit: "ms", samples: [raw.measurements.uninstallMs] },
  },
  resources,
  assertions: [
    `${raw.installerKind} installed into an ephemeral VM`,
    `${expectedSampling.sampleCount} cold and warm native-window launches completed`,
    `the installed process tree was sampled for ${expectedSampling.idleWindowSeconds} seconds`,
    `${raw.installerKind} uninstalled without leaving its executable`,
  ],
});
await writeFile(options.output, `${JSON.stringify(report, null, 2)}\n`);
process.stdout.write(
  `installed bundle performance report: ${options.output}\n`,
);

function parseArguments(arguments_) {
  const values = {};
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      throw new Error(`invalid bundle report argument: ${name ?? ""}`);
    }
    values[name.slice(2)] = value;
  }
  for (const required of ["raw", "output", "commit", "baseline-commit"]) {
    if (!values[required]) throw new Error(`--${required} is required`);
  }
  for (const name of ["commit", "baseline-commit"]) {
    if (!/^[0-9a-f]{40}$/.test(values[name])) {
      throw new Error(`--${name} must be a full lowercase Git commit`);
    }
  }
  return {
    raw: path.resolve(values.raw),
    output: path.resolve(values.output),
    commit: values.commit,
    baselineCommit: values["baseline-commit"],
    runId: values["run-id"] ?? process.env.GITHUB_RUN_ID ?? "local",
  };
}

function normalizeSnapshot(snapshot) {
  const value = (lower, upper) => snapshot[lower] ?? snapshot[upper];
  return {
    processCount: Number(value("processCount", "ProcessCount")),
    privateMemoryBytes: Number(
      value("privateMemoryBytes", "PrivateMemoryBytes"),
    ),
    workingSetBytes: Number(value("workingSetBytes", "WorkingSetBytes")),
    cpuSeconds: Number(value("cpuSeconds", "CpuSeconds")),
  };
}

function runnerImage() {
  const image = process.env.ImageOS ?? process.env.RUNNER_OS ?? "local";
  const version = process.env.ImageVersion ?? os.release();
  return `${image}-${version}`;
}

function powershell(script) {
  return execFileSync(
    "powershell.exe",
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    { encoding: "utf8", windowsHide: true },
  ).trim();
}
