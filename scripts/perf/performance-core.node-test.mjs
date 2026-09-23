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
const idlePolicy = validatePerformancePolicy(
  JSON.parse(
    await readFile(
      new URL("../../performance/budgets.idle.json", import.meta.url),
    ),
  ),
);
const latencyPolicy = validatePerformancePolicy(
  JSON.parse(
    await readFile(
      new URL("../../performance/budgets.latency.json", import.meta.url),
    ),
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

test("a metric-specific noise floor overrides its suite unit floor", () => {
  const testPolicy = structuredClone(policy);
  const suite = testPolicy.platforms.windows.suites["installed-bundle"];
  suite.relativeNoiseFloor = { ms: 0, bytes: 0, percent: 0, count: 0 };
  for (const budget of Object.values(suite.metrics)) {
    budget.absoluteMax = 1000;
    budget.relativeMaxPercent = 30;
  }
  suite.metrics.coldStartupMs.relativeMaxPercent = 20;
  suite.metrics.coldStartupMs.relativeNoiseFloor = 30;

  const baseline = makeReport({ commit: baselineCommit, sampleValue: 100 });
  const candidate = makeReport({ commit: candidateCommit, sampleValue: 125 });
  const gate = evaluatePerformance(candidate, baseline, testPolicy);
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === "coldStartupMs",
  );

  assert.equal(gate.status, "passed");
  assert.equal(comparison.relativeNoiseFloor, 30);
  assert.equal(comparison.relativeLimit, 130);
});

// #181: the Linux idle CPU-percentage relative gate failed on commits whose
// product source was byte-identical. Twelve product-unchanged runs moved the
// candidate-versus-baseline p95 by up to 3.41 percentage points, which the
// shared 1-point floor read as a 60-113 percent regression.
test("the Linux idle CPU floor absorbs the measured cross-VM runner spread", () => {
  // Run 35734525535, the failure quoted in #181: 2.297 -> 3.697 percent.
  const baseline = makeIdleReport({
    commit: baselineCommit,
    cpuPercent: 2.297,
  });
  const candidate = makeIdleReport({
    commit: candidateCommit,
    cpuPercent: 3.697,
  });
  const gate = evaluatePerformance(candidate, baseline, idlePolicy);

  assert.equal(gate.status, "passed");
  for (const metric of ["idleCpuPercent", "backgroundPollingCpuPercent"]) {
    const comparison = gate.comparisons.find((item) => item.metric === metric);
    assert.equal(comparison.relativeNoiseFloor, 2);
    // 2.297 + max(2.297 * 20%, 2) = 4.297, above the observed 3.697.
    assert.equal(comparison.relativeLimit, 4.297);
    assert.equal(comparison.passed, true);
  }
});

test("the Linux idle CPU floor fails one step past its boundary", () => {
  const baseline = makeIdleReport({ commit: baselineCommit, cpuPercent: 2 });
  // 2 + max(0.4, 2) = 4 exactly.
  const atLimit = makeIdleReport({ commit: candidateCommit, cpuPercent: 4 });
  assert.equal(
    evaluatePerformance(atLimit, baseline, idlePolicy).status,
    "passed",
  );

  const pastLimit = makeIdleReport({
    commit: candidateCommit,
    cpuPercent: 4.001,
  });
  const gate = evaluatePerformance(pastLimit, baseline, idlePolicy);
  assert.equal(gate.status, "failed");
  assert(
    gate.violations.every(
      (violation) =>
        violation.kind === "relative" &&
        violation.limit === 4 &&
        violation.actual === 4.001,
    ),
  );
});

// The floor must not become a way to smuggle a real regression past the gate.
test("the 8 percent absolute idle rail still fires above the relative floor", () => {
  // 7 + max(1.4, 2) = 9, so the relative gate is satisfied and only the
  // absolute rail can reject this candidate.
  const baseline = makeIdleReport({ commit: baselineCommit, cpuPercent: 7 });
  const candidate = makeIdleReport({
    commit: candidateCommit,
    cpuPercent: 8.5,
  });
  const gate = evaluatePerformance(candidate, baseline, idlePolicy);

  assert.equal(gate.status, "failed");
  const cpuViolations = gate.violations.filter((violation) =>
    violation.metric.endsWith("CpuPercent"),
  );
  assert.equal(cpuViolations.length, 2);
  assert(
    cpuViolations.every(
      (violation) => violation.kind === "absolute" && violation.limit === 8,
    ),
  );
});

// Thirteen Windows runs moved by at most 0.315 percentage points, so the
// Windows floors stay where they are.
test("Windows idle keeps the shared one point floor", () => {
  const metrics =
    idlePolicy.platforms.windows.suites["release-webview"].metrics;
  for (const metric of ["idleCpuPercent", "backgroundPollingCpuPercent"]) {
    assert.equal(metrics[metric].relativeNoiseFloor, undefined);
  }

  const baseline = makeIdleReport({
    commit: baselineCommit,
    cpuPercent: 2.3,
    platform: "windows",
  });
  const candidate = makeIdleReport({
    commit: candidateCommit,
    cpuPercent: 3.7,
    platform: "windows",
  });
  const gate = evaluatePerformance(candidate, baseline, idlePolicy);

  assert.equal(gate.status, "failed");
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === "idleCpuPercent",
  );
  assert.equal(comparison.relativeNoiseFloor, 1);
  assert.equal(comparison.relativeLimit, 3.3);
});

// #185: nearest-rank p95 at n=3 is the slowest of three samples. For
// environmentTimeoutRecoveryMs the slowest sample is not a distribution tail:
// across twenty-six Linux sample sets the second post-warm-up sample was the
// maximum twenty-two times. The relative gate therefore compared two draws of
// one reproducible blip. It now compares medians while the absolute rail keeps
// reading p95.
const latencyRecovery = "environmentTimeoutRecoveryMs";
// #193: the same dual-statistic mechanism, applied to a different pathology.
// navigationMs walks a three-route list once each, so sample index selects the
// route rather than repeating one workload, and nearest-rank p95 at n=3 reads
// a single sample of the most expensive route.
const latencyNavigation = "navigationMs";
const latencySlow = "environmentSlowMs";

test("a budget without relativeStatistic compares on its absolute statistic", () => {
  const baseline = makeLatencyReport({ commit: baselineCommit });
  const candidate = makeLatencyReport({ commit: candidateCommit });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);

  assert.equal(gate.status, "passed");
  for (const comparison of gate.comparisons) {
    if (comparison.metric === latencyRecovery) continue;
    if (comparison.metric === latencyNavigation) continue;
    assert.equal(comparison.relativeStatistic, comparison.statistic);
    assert.equal(comparison.relativeActual, comparison.actual);
    assert.equal(comparison.relativeBaseline, comparison.baseline);
  }
});

test("the Linux recovery gate passes the run 35739384785 false positive", () => {
  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyRecovery]: [1398.505, 1653.319, 1427.885] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyRecovery]: [1342.319, 3165.508, 1314.503] },
  });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);

  assert.equal(gate.status, "passed");
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === latencyRecovery,
  );
  // The absolute rail still reads p95 and still has room.
  assert.equal(comparison.statistic, "p95");
  assert.equal(comparison.actual, 3165.508);
  assert.equal(comparison.absoluteLimit, 6000);
  // The relative comparison reads p50, where the candidate is faster.
  assert.equal(comparison.relativeStatistic, "p50");
  assert.equal(comparison.relativeActual, 1342.319);
  assert.equal(comparison.relativeBaseline, 1427.885);
  assert.equal(comparison.relativeLimit, 1713.462);
  assert(comparison.relativeChangePercent < 0);
});

test("the same run still fails when the relative gate reads p95", () => {
  const testPolicy = structuredClone(latencyPolicy);
  delete testPolicy.platforms.linux.suites["release-webview"].metrics[
    latencyRecovery
  ].relativeStatistic;

  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyRecovery]: [1398.505, 1653.319, 1427.885] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyRecovery]: [1342.319, 3165.508, 1314.503] },
  });
  const gate = evaluatePerformance(candidate, baseline, testPolicy);

  assert.equal(gate.status, "failed");
  assert.deepEqual(
    gate.violations.map(({ metric, kind, statistic }) => ({
      metric,
      kind,
      statistic,
    })),
    [{ metric: latencyRecovery, kind: "relative", statistic: "p95" }],
  );
});

// The dual statistic must not become a way to hide a tail regression.
test("the p95 absolute rail still fires while the relative gate reads p50", () => {
  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyRecovery]: [1398.505, 1653.319, 1427.885] },
  });
  // p50 is 100 ms and passes the relative gate; p95 is 6500 ms and cannot pass
  // the 6000 ms rail.
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyRecovery]: [100, 6500, 100] },
  });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);

  assert.equal(gate.status, "failed");
  assert.deepEqual(
    gate.violations.map(({ metric, kind, statistic, actual, limit }) => ({
      metric,
      kind,
      statistic,
      actual,
      limit,
    })),
    [
      {
        metric: latencyRecovery,
        kind: "absolute",
        statistic: "p95",
        actual: 6500,
        limit: 6000,
      },
    ],
  );
});

test("a relative violation reports the statistic its gate actually used", () => {
  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyRecovery]: [1398.505, 1653.319, 1427.885] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyRecovery]: [1900, 2000, 2100] },
  });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);

  assert.equal(gate.status, "failed");
  const violation = gate.violations.find(
    ({ metric }) => metric === latencyRecovery,
  );
  assert.equal(violation.kind, "relative");
  assert.equal(violation.statistic, "p50");
  assert.equal(violation.actual, 2000);
  assert.equal(violation.baseline, 1427.885);
  assert.equal(violation.limit, 1713.462);

  const markdown = renderPerformanceMarkdown(candidate);
  assert.match(markdown, /abs p95: 2100\.00 ms; rel p50: 2000\.00 ms/);
  assert.match(markdown, /abs 1653\.32 ms; rel 1427\.88 ms/);
  assert.match(markdown, new RegExp(`${latencyRecovery} p50: relative`));
});

test("Windows recovery keeps a single statistic for both gates", () => {
  const metrics =
    latencyPolicy.platforms.windows.suites["release-webview"].metrics;
  assert.equal(metrics[latencyRecovery].relativeStatistic, undefined);

  const baseline = makeLatencyReport({
    commit: baselineCommit,
    platform: "windows",
    sampleLists: { [latencyRecovery]: [760, 770, 780] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    platform: "windows",
    sampleLists: { [latencyRecovery]: [900, 1000, 1100] },
  });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === latencyRecovery,
  );

  assert.equal(comparison.statistic, "p95");
  assert.equal(comparison.relativeStatistic, "p95");
  assert.equal(comparison.actual, 1100);
  assert.equal(comparison.relativeActual, 1100);
  assert.equal(gate.status, "failed");
});

test("an unsupported relative statistic is rejected", () => {
  const testPolicy = structuredClone(latencyPolicy);
  testPolicy.platforms.linux.suites["release-webview"].metrics[
    latencyRecovery
  ].relativeStatistic = "mean";

  const baseline = makeLatencyReport({ commit: baselineCommit });
  const candidate = makeLatencyReport({ commit: candidateCommit });
  // The budget schema rejects it first; the engine guard behind it mirrors the
  // existing guard on `statistic` and stays as defence in depth.
  assert.throws(
    () => evaluatePerformance(candidate, baseline, testPolicy),
    /relativeStatistic must be equal to one of the allowed values/,
  );
  assert.throws(
    () => validatePerformancePolicy(testPolicy),
    /relativeStatistic must be equal to one of the allowed values/,
  );
});

test("the Linux navigation gate passes the run 35703788546 false positive", () => {
  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyNavigation]: [116.839, 132.868, 59.273] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyNavigation]: [76.459, 266.632, 67.722] },
  });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);

  assert.equal(gate.status, "passed");
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === latencyNavigation,
  );
  // The absolute rail still reads p95 and still sees the 266.632 ms sample.
  assert.equal(comparison.statistic, "p95");
  assert.equal(comparison.actual, 266.632);
  assert.equal(comparison.absoluteLimit, 1500);
  // The relative comparison reads p50, where the candidate is faster: the
  // median route went from 116.839 ms to 76.459 ms while a single Settings
  // sample spiked.
  assert.equal(comparison.relativeStatistic, "p50");
  assert.equal(comparison.relativeActual, 76.459);
  assert.equal(comparison.relativeBaseline, 116.839);
  assert(comparison.relativeChangePercent < 0);
});

test("the same navigation run still fails when the relative gate reads p95", () => {
  const testPolicy = structuredClone(latencyPolicy);
  delete testPolicy.platforms.linux.suites["release-webview"].metrics[
    latencyNavigation
  ].relativeStatistic;

  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyNavigation]: [116.839, 132.868, 59.273] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyNavigation]: [76.459, 266.632, 67.722] },
  });
  const gate = evaluatePerformance(candidate, baseline, testPolicy);

  assert.equal(gate.status, "failed");
  assert.deepEqual(
    gate.violations.map(({ metric, kind, statistic }) => ({
      metric,
      kind,
      statistic,
    })),
    [{ metric: latencyNavigation, kind: "relative", statistic: "p95" }],
  );
});

// The coverage that p50 gives up must stay explicit. A regression confined to
// the most expensive route no longer moves the relative gate, so the 1500 ms
// rail is the only thing left holding it.
test("a navigation regression confined to the slowest route reaches only the rail", () => {
  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencyNavigation]: [116.839, 132.868, 59.273] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyNavigation]: [116.9, 1400, 59.3] },
  });
  const gate = evaluatePerformance(candidate, baseline, latencyPolicy);

  assert.equal(gate.status, "passed");
  const comparison = gate.comparisons.find(
    ({ metric }) => metric === latencyNavigation,
  );
  assert.equal(comparison.actual, 1400);
  assert.equal(comparison.relativeActual, 116.9);

  // Past the rail it does fail, and it fails on the absolute statistic.
  const overRail = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencyNavigation]: [116.9, 1600, 59.3] },
  });
  const railed = evaluatePerformance(overRail, baseline, latencyPolicy);
  assert.equal(railed.status, "failed");
  assert.deepEqual(
    railed.violations.map(({ metric, kind, statistic }) => ({
      metric,
      kind,
      statistic,
    })),
    [{ metric: latencyNavigation, kind: "absolute", statistic: "p95" }],
  );
});

// #193: environmentSlowMs looks like the same bug and is not. Its maximum
// falls on the *first* sample in thirty of thirty-eight Linux sample sets,
// which is a missing warm-up rather than a tail artifact, and the data shows
// that swapping the relative statistic does not fix it. The harness gained a
// discarded warm-up probe instead; this test pins the budget so the
// ineffective remedy is not applied later by analogy with navigationMs.
test("the slow-environment gate keeps a single statistic and is not fixed by p50", () => {
  const metrics =
    latencyPolicy.platforms.linux.suites["release-webview"].metrics;
  assert.equal(metrics[latencySlow].relativeStatistic, undefined);

  // Run 35715515352, the residual failure that the warm-up does not remove.
  const baseline = makeLatencyReport({
    commit: baselineCommit,
    sampleLists: { [latencySlow]: [1151.738, 944.159, 828.244] },
  });
  const candidate = makeLatencyReport({
    commit: candidateCommit,
    sampleLists: { [latencySlow]: [1360.191, 1632.43, 884.534] },
  });
  assert.equal(
    evaluatePerformance(candidate, baseline, latencyPolicy).status,
    "failed",
  );

  const p50Policy = structuredClone(latencyPolicy);
  p50Policy.platforms.linux.suites["release-webview"].metrics[
    latencySlow
  ].relativeStatistic = "p50";
  assert.equal(
    evaluatePerformance(candidate, baseline, p50Policy).status,
    "failed",
  );
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

// Only the CPU-percentage metrics vary in the idle fixtures below; every other
// metric is pinned well inside its budget so a failing gate is unambiguous.
function idleMetricDefault(name, cpuPercent) {
  if (metricUnit(name) === "percent") return cpuPercent;
  if (name === "processCount") return 5;
  return defaultMetricValue(name, 100);
}

// Release-webview idle reports, which is where the CPU-percentage relative
// gate produced the #181 false positives. Unlike the latency phase, the idle
// phase measures one variant per job, so a baseline and a candidate report
// come from two different runner VMs.
function makeIdleReport({
  commit,
  cpuPercent,
  platform = "linux",
  metricValues = {},
}) {
  return makeReleaseWebviewReport({
    commit,
    platform,
    defaultValue: (name) => idleMetricDefault(name, cpuPercent),
    metricValues,
  });
}

// A latency-phase release-webview report. `sampleLists` supplies the raw
// samples for a metric so a test can reproduce a real run's distribution
// instead of a flat fill.
function makeLatencyReport({
  commit,
  platform = "linux",
  sampleCount = 3,
  sampleLists = {},
  metricValues = {},
}) {
  return makeReleaseWebviewReport({
    commit,
    platform,
    sampleCount,
    sampleLists,
    defaultValue: (name) => idleMetricDefault(name, 1),
    metricValues,
  });
}

function makeReleaseWebviewReport({
  commit,
  platform = "linux",
  sampleCount = 7,
  sampleLists = {},
  defaultValue,
  metricValues = {},
}) {
  // A release-webview report always carries the full metric set; each phase's
  // budget file only gates the subset it measured.
  const metricNames = [
    ...new Set([
      ...Object.keys(
        latencyPolicy.platforms[platform].suites["release-webview"].metrics,
      ),
      ...Object.keys(
        idlePolicy.platforms[platform].suites["release-webview"].metrics,
      ),
    ]),
  ];
  const isLinux = platform === "linux";
  return createPerformanceReport({
    benchmark: {
      suite: "release-webview",
      platform,
      buildProfile: "release",
      packageKind: isLinux ? "appimage" : "release-executable",
      commit,
      baselineCommit,
      startedAt: "2026-09-22T00:00:00.000Z",
      finishedAt: "2026-09-22T00:05:00.000Z",
      runId: "unit-test-release-webview",
    },
    host: {
      os: platform,
      osVersion: isLinux ? "Linux test" : "Windows 11 test",
      arch: "x64",
      runnerImage: isLinux ? "ubuntu24-test" : "windows-test",
      cpuModel: "test CPU",
      logicalCores: 4,
      memoryBytes: 16 * 1024 ** 3,
      memoryClassBytes: 16 * 1024 ** 3,
      webviewVersion: isLinux ? "2.52.6" : "151.0.0.0",
    },
    fixture: {
      workspaceTiers: {
        small: { projectCount: 1, totalBytes: 1103 },
        large: { projectCount: 250, totalBytes: 65555750 },
      },
      catalogModes: ["cached", "isolated-refresh"],
      environmentModes: ["normal", "slow", "timeout-recovery"],
    },
    sampling: samplingPolicy({
      sampleCount,
      idleWindowSeconds: 300,
      idleSampleSeconds: 5,
    }),
    metricSamples: Object.fromEntries(
      metricNames.map((name) => [
        name,
        {
          unit: metricUnit(name),
          samples:
            sampleLists[name] ??
            Array(metricSampleCount(name, sampleCount)).fill(
              metricValues[name] ?? defaultValue(name),
            ),
        },
      ]),
    ),
    resources: resourceSummary(snapshot(), snapshot(), snapshot()),
    assertions: ["unit fixture"],
  });
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

function metricSampleCount(name, sampleCount = 7) {
  if (
    name.endsWith("GrowthBytes") ||
    name === "processCountGrowth" ||
    name === "installMs" ||
    name === "uninstallMs"
  ) {
    return 1;
  }
  // Mirrors expectedSampleCount: the polling metric is capped at 12 samples.
  if (name === "backgroundPollingCpuPercent") {
    return 12;
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
  return sampleCount;
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
