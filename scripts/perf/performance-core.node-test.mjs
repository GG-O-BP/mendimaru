import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  assertCompatibleReports,
  createPerformanceReport,
  createProcessCpuTracker,
  evaluatePerformance,
  median,
  nearestRank,
  normalizedCpuPercent,
  renderPerformanceMarkdown,
  resourceSummary,
  samplingPolicy,
  summarizeSamples,
  trackProcessCpuSeconds,
  validatePerformancePolicy,
  validatePerformanceReport,
  validateProcessTree,
} from "./performance-core.mjs";

const policy = validatePerformancePolicy(
  JSON.parse(
    await readFile(new URL("../../performance/budgets.json", import.meta.url)),
  ),
);
const baselineCommit = "1".repeat(40);
const candidateCommit = "2".repeat(40);

test("nearest-rank percentiles and dispersion retain reported outliers", () => {
  const samples = [10, 11, 12, 13, 100];
  assert.equal(nearestRank(samples, 50), 12);
  assert.equal(nearestRank(samples, 95), 100);
  assert.equal(median([1, 2, 8, 9]), 5);
  assert.deepEqual(summarizeSamples(samples, "ms"), {
    unit: "ms",
    samples,
    sampleCount: 5,
    min: 10,
    max: 100,
    p50: 12,
    p95: 100,
    medianAbsoluteDeviation: 1,
    iqr: 2,
    outlierIndices: [4],
  });
});

test("metrics reject missing, NaN, infinite, and negative samples", () => {
  for (const samples of [[], [Number.NaN], [Number.POSITIVE_INFINITY], [-1]]) {
    assert.throws(() => summarizeSamples(samples, "ms"), /metric sample/);
  }
});

test("process trees reject duplicate, missing-root, and invalid records", () => {
  assert.deepEqual(
    validateProcessTree(
      [
        { pid: 10, parentPid: 1 },
        { pid: 11, parentPid: 10 },
        { pid: 12, parentPid: 11 },
        { pid: 90, parentPid: 1 },
      ],
      10,
    ),
    [10, 11, 12],
  );
  assert.throws(
    () =>
      validateProcessTree(
        [
          { pid: 10, parentPid: 1 },
          { pid: 10, parentPid: 1 },
        ],
        10,
      ),
    /duplicate PID/,
  );
  assert.throws(
    () => validateProcessTree([{ pid: 11, parentPid: 10 }], 10),
    /missing root/,
  );
  assert.throws(
    () => validateProcessTree([{ pid: -1, parentPid: 0 }], 10),
    /invalid PID/,
  );
  assert.throws(
    () =>
      validateProcessTree(
        [
          { pid: 10, parentPid: 11 },
          { pid: 11, parentPid: 10 },
        ],
        10,
      ),
    /cycle/,
  );
});

test("resource and CPU fixtures expose child, memory, and sustained CPU growth", () => {
  const resources = resourceSummary(
    snapshot({
      processCount: 2,
      privateMemoryBytes: 100,
      workingSetBytes: 200,
    }),
    snapshot({
      processCount: 4,
      privateMemoryBytes: 180,
      workingSetBytes: 320,
      cpuSeconds: 4,
    }),
    snapshot({
      processCount: 4,
      privateMemoryBytes: 190,
      workingSetBytes: 350,
      cpuSeconds: 4,
    }),
  );
  assert.deepEqual(resources.delta, {
    processCount: 2,
    privateMemoryBytes: 80,
    workingSetBytes: 120,
  });
  assert.equal(
    normalizedCpuPercent({
      beforeCpuSeconds: 0,
      afterCpuSeconds: 4,
      elapsedSeconds: 10,
      logicalCores: 2,
    }),
    20,
  );
  assert.throws(
    () =>
      normalizedCpuPercent({
        beforeCpuSeconds: 4,
        afterCpuSeconds: 3,
        elapsedSeconds: 10,
        logicalCores: 2,
      }),
    /moved backwards/,
  );
});

test("process CPU tracking remains monotonic when short-lived children exit", () => {
  const tracker = createProcessCpuTracker();
  assert.equal(
    trackProcessCpuSeconds(tracker, [
      { identity: "root:1", cpuSeconds: 10 },
      { identity: "child:1", cpuSeconds: 2 },
    ]),
    0,
  );
  assert.equal(
    trackProcessCpuSeconds(tracker, [
      { identity: "root:1", cpuSeconds: 11 },
      { identity: "child:1", cpuSeconds: 3 },
      { identity: "short:1", cpuSeconds: 0.5 },
    ]),
    2.5,
  );
  assert.equal(
    trackProcessCpuSeconds(tracker, [{ identity: "root:1", cpuSeconds: 12 }]),
    3.5,
  );
});

test("performance report schema rejects missing and inconsistent metrics", () => {
  const report = makeReport({ commit: candidateCommit });
  assert.equal(validatePerformanceReport(report), report);

  const missing = structuredClone(report);
  delete missing.host.cpuModel;
  assert.throws(() => validatePerformanceReport(missing), /cpuModel/);

  const missingMetric = structuredClone(report);
  delete missingMetric.metrics.coldStartupMs;
  assert.throws(
    () => validatePerformanceReport(missingMetric),
    /missing metric coldStartupMs/,
  );

  const nan = structuredClone(report);
  nan.metrics.coldStartupMs.samples[0] = Number.NaN;
  assert.throws(() => validatePerformanceReport(nan), /schema/);

  const negative = structuredClone(report);
  negative.metrics.coldStartupMs.samples[0] = -1;
  assert.throws(() => validatePerformanceReport(negative), /schema/);

  const inconsistent = structuredClone(report);
  inconsistent.metrics.coldStartupMs.p95 += 1;
  assert.throws(() => validatePerformanceReport(inconsistent), /raw samples/);
});

test("report comparison rejects baseline commit and host metadata mismatch", () => {
  const baseline = makeReport({ commit: baselineCommit });
  const candidate = makeReport({ commit: candidateCommit });
  assert.equal(assertCompatibleReports(candidate, baseline, policy), true);

  const wrongCommit = structuredClone(candidate);
  wrongCommit.benchmark.baselineCommit = "3".repeat(40);
  wrongCommit.gate.baselineCommit = "3".repeat(40);
  assert.throws(
    () => assertCompatibleReports(wrongCommit, baseline, policy),
    /baseline commit/,
  );

  const wrongHost = structuredClone(candidate);
  wrongHost.host.logicalCores = 16;
  assert.throws(
    () => assertCompatibleReports(wrongHost, baseline, policy),
    /logicalCores/,
  );
});

test("20 percent boundary passes and representative 25 percent regression fails", () => {
  const testPolicy = structuredClone(policy);
  for (const budget of Object.values(
    testPolicy.platforms.windows.suites["installed-bundle"].metrics,
  )) {
    budget.absoluteMax = 1000;
    budget.relativeMaxPercent = 20;
  }
  testPolicy.platforms.windows.suites["installed-bundle"].relativeNoiseFloor = {
    ms: 0,
    bytes: 0,
    percent: 0,
    count: 0,
  };
  const baseline = makeReport({ commit: baselineCommit, sampleValue: 100 });
  const boundary = makeReport({ commit: candidateCommit, sampleValue: 120 });
  const passingGate = evaluatePerformance(boundary, baseline, testPolicy);
  assert.equal(passingGate.status, "passed");
  assert.equal(boundary.status, "passed");

  const regression = makeReport({ commit: candidateCommit, sampleValue: 125 });
  const failingGate = evaluatePerformance(regression, baseline, testPolicy);
  assert.equal(failingGate.status, "failed");
  assert.equal(regression.status, "failed");
  assert.match(regression.error, /performance gate failed/);
  assert(
    failingGate.violations.every(
      (violation) =>
        violation.kind === "relative" && violation.relativeChangePercent === 25,
    ),
  );
  const markdown = renderPerformanceMarkdown(regression);
  assert.match(markdown, /Candidate p50/);
  assert.match(markdown, /\| coldStartupMs \| 7 \|/);
  assert.match(markdown, /25\.00%/);
});

test("relative noise floor permits a bounded delta from a zero baseline", () => {
  const baseline = makeReport({ commit: baselineCommit, sampleValue: 1 });
  const candidate = makeReport({
    commit: candidateCommit,
    sampleValue: 1,
    metricValues: { processCountGrowth: 1 },
    resources: growthResources({
      processCount: 1,
      privateMemoryBytes: 0,
      workingSetBytes: 0,
    }),
  });
  const gate = evaluatePerformance(candidate, baseline, policy);
  assert.equal(gate.status, "passed");
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === "processCountGrowth",
  );
  assert.equal(comparison.relativeNoiseFloor, 2);
  assert.equal(comparison.relativeLimit, 2);
});

test("child, memory, and sustained CPU leak fixtures fail their budgets", () => {
  const baseline = makeReport({
    commit: baselineCommit,
    sampleValue: 1,
    metricValues: {
      processCountGrowth: 1,
      privateMemoryGrowthBytes: 32 * 1024 ** 2,
      workingSetGrowthBytes: 32 * 1024 ** 2,
      idleCpuPercent: 4,
    },
    resources: growthResources({
      processCount: 1,
      privateMemoryBytes: 32 * 1024 ** 2,
      workingSetBytes: 32 * 1024 ** 2,
    }),
  });
  const candidate = makeReport({
    commit: candidateCommit,
    sampleValue: 1,
    metricValues: {
      processCountGrowth: 3,
      privateMemoryGrowthBytes: 110 * 1024 ** 2,
      workingSetGrowthBytes: 180 * 1024 ** 2,
      idleCpuPercent: 9,
    },
    resources: growthResources({
      processCount: 3,
      privateMemoryBytes: 110 * 1024 ** 2,
      workingSetBytes: 180 * 1024 ** 2,
    }),
  });
  const gate = evaluatePerformance(candidate, baseline, policy);
  for (const metric of [
    "processCountGrowth",
    "privateMemoryGrowthBytes",
    "workingSetGrowthBytes",
    "idleCpuPercent",
  ]) {
    assert(
      gate.violations.some(
        (violation) =>
          violation.metric === metric && violation.kind === "absolute",
      ),
      `${metric} must have an absolute violation`,
    );
  }
});

function makeReport({
  commit,
  sampleValue = 100,
  metricValues = {},
  resources = resourceSummary(snapshot(), snapshot(), snapshot()),
}) {
  const metricNames = Object.keys(
    policy.platforms.windows.suites["installed-bundle"].metrics,
  );
  metricNames.push("installMs", "uninstallMs");
  return createPerformanceReport({
    benchmark: {
      suite: "installed-bundle",
      platform: "windows",
      buildProfile: "release",
      packageKind: "msi",
      commit,
      baselineCommit,
      startedAt: "2026-08-26T00:00:00.000Z",
      finishedAt: "2026-08-26T00:05:00.000Z",
      runId: "unit-test",
    },
    host: {
      os: "windows",
      osVersion: "Windows 11 test",
      arch: "x64",
      runnerImage: "windows-test",
      cpuModel: "test CPU",
      logicalCores: 8,
      memoryBytes: 16 * 1024 ** 3,
      memoryClassBytes: 16 * 1024 ** 3,
      webviewVersion: "151.0.0.0",
    },
    fixture: {
      workspaceTiers: null,
      catalogModes: [],
      environmentModes: [],
    },
    sampling: samplingPolicy(),
    metricSamples: Object.fromEntries(
      metricNames.map((name) => [
        name,
        {
          unit: metricUnit(name),
          samples: Array(metricSampleCount(name)).fill(
            metricValues[name] ?? defaultMetricValue(name, sampleValue),
          ),
        },
      ]),
    ),
    resources,
    assertions: ["unit fixture"],
  });
}

function defaultMetricValue(name, sampleValue) {
  return name.endsWith("GrowthBytes") || name === "processCountGrowth"
    ? 0
    : sampleValue;
}

function growthResources(delta) {
  const before = snapshot();
  const after = snapshot({
    processCount: before.processCount + delta.processCount,
    privateMemoryBytes: before.privateMemoryBytes + delta.privateMemoryBytes,
    workingSetBytes: before.workingSetBytes + delta.workingSetBytes,
  });
  return resourceSummary(before, after, after);
}

function metricSampleCount(name) {
  if (
    name.endsWith("GrowthBytes") ||
    name === "processCountGrowth" ||
    name === "installMs" ||
    name === "uninstallMs"
  ) {
    return 1;
  }
  if (
    [
      "idleCpuPercent",
      "privateMemoryBytes",
      "workingSetBytes",
      "processCount",
    ].includes(name)
  ) {
    return 60;
  }
  return 7;
}

function metricUnit(name) {
  if (name.endsWith("Ms")) return "ms";
  if (name.endsWith("Bytes")) return "bytes";
  if (name.endsWith("Percent")) return "percent";
  return "count";
}

function snapshot(overrides = {}) {
  return {
    processCount: 2,
    privateMemoryBytes: 100,
    workingSetBytes: 200,
    cpuSeconds: 0,
    ...overrides,
  };
}
