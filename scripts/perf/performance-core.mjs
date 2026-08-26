import { readFileSync } from "node:fs";
import { cpus, totalmem } from "node:os";
import { fileURLToPath } from "node:url";

import Ajv from "ajv";
import addFormats from "ajv-formats";

const REPORT_SCHEMA_VERSION = "1.0.0";
const ROUNDING_DIGITS = 3;
const REQUIRED_METRICS = {
  "release-webview": [
    "coldStartupMs",
    "warmStartupMs",
    "firstIpcMs",
    "environmentSlowMs",
    "environmentTimeoutRecoveryMs",
    "catalogCachedMs",
    "catalogRefreshMs",
    "smallWorkspaceScanMs",
    "largeWorkspaceScanMs",
    "navigationMs",
    "backgroundPollingCpuPercent",
    "idleCpuPercent",
    "privateMemoryBytes",
    "workingSetBytes",
    "processCount",
    "privateMemoryGrowthBytes",
    "workingSetGrowthBytes",
    "processCountGrowth",
  ],
  "installed-bundle": [
    "coldStartupMs",
    "warmStartupMs",
    "idleCpuPercent",
    "privateMemoryBytes",
    "workingSetBytes",
    "processCount",
    "privateMemoryGrowthBytes",
    "workingSetGrowthBytes",
    "processCountGrowth",
    "installMs",
    "uninstallMs",
  ],
};
const schemaPath = fileURLToPath(
  new URL("../../schemas/performance-report.schema.json", import.meta.url),
);
const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
const policySchemaPath = fileURLToPath(
  new URL("../../schemas/performance-budget.schema.json", import.meta.url),
);
const policySchema = JSON.parse(readFileSync(policySchemaPath, "utf8"));
const ajv = new Ajv({ allErrors: true, strict: true });
addFormats(ajv);
const validateSchema = ajv.compile(schema);
const validatePolicySchema = ajv.compile(policySchema);

export const performanceReportSchemaVersion = REPORT_SCHEMA_VERSION;

export function nearestRank(samples, percentile) {
  const values = checkedSamples(samples);
  if (!Number.isFinite(percentile) || percentile <= 0 || percentile > 100) {
    throw new Error("percentile must be finite and within (0, 100]");
  }
  const sorted = values.toSorted((left, right) => left - right);
  return sorted[Math.ceil((percentile / 100) * sorted.length) - 1];
}

export function median(samples) {
  const sorted = checkedSamples(samples).toSorted(
    (left, right) => left - right,
  );
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

export function summarizeSamples(samples, unit) {
  if (!["ms", "bytes", "percent", "count"].includes(unit)) {
    throw new Error(`unsupported metric unit: ${unit}`);
  }
  const values = checkedSamples(samples);
  const center = median(values);
  const q1 = nearestRank(values, 25);
  const q3 = nearestRank(values, 75);
  const iqr = q3 - q1;
  const lowerFence = q1 - iqr * 1.5;
  const upperFence = q3 + iqr * 1.5;
  const outlierIndices = values.flatMap((value, index) =>
    value < lowerFence || value > upperFence ? [index] : [],
  );
  return {
    unit,
    samples: values.map(rounded),
    sampleCount: values.length,
    min: rounded(Math.min(...values)),
    max: rounded(Math.max(...values)),
    p50: rounded(nearestRank(values, 50)),
    p95: rounded(nearestRank(values, 95)),
    medianAbsoluteDeviation: rounded(
      median(values.map((value) => Math.abs(value - center))),
    ),
    iqr: rounded(iqr),
    outlierIndices,
  };
}

export function validateProcessTree(records, rootPid) {
  if (!Number.isInteger(rootPid) || rootPid <= 0) {
    throw new Error("process tree root PID must be a positive integer");
  }
  if (!Array.isArray(records) || records.length === 0) {
    throw new Error("process tree records must be a non-empty array");
  }
  const byPid = new Map();
  for (const record of records) {
    if (
      !record ||
      !Number.isInteger(record.pid) ||
      record.pid <= 0 ||
      !Number.isInteger(record.parentPid) ||
      record.parentPid < 0
    ) {
      throw new Error("process tree contains an invalid PID record");
    }
    if (byPid.has(record.pid)) {
      throw new Error(`process tree contains duplicate PID ${record.pid}`);
    }
    byPid.set(record.pid, record);
  }
  if (!byPid.has(rootPid)) {
    throw new Error(`process tree is missing root PID ${rootPid}`);
  }

  const selected = new Set([rootPid]);
  let changed;
  do {
    changed = false;
    for (const record of records) {
      if (selected.has(record.parentPid) && !selected.has(record.pid)) {
        selected.add(record.pid);
        changed = true;
      }
    }
  } while (changed);

  for (const pid of selected) {
    const visited = new Set();
    let current = pid;
    while (true) {
      if (visited.has(current)) {
        throw new Error(`process tree contains a cycle at PID ${current}`);
      }
      visited.add(current);
      if (current === rootPid) {
        const rootParent = byPid.get(rootPid).parentPid;
        if (selected.has(rootParent)) {
          throw new Error(`process tree contains a cycle at PID ${rootPid}`);
        }
        break;
      }
      const record = byPid.get(current);
      if (!record || !selected.has(record.parentPid)) {
        throw new Error(`process tree PID ${pid} is not rooted at ${rootPid}`);
      }
      current = record.parentPid;
    }
  }
  return [...selected].toSorted((left, right) => left - right);
}

export function resourceSummary(before, after, peak) {
  for (const [name, snapshot] of Object.entries({ before, after, peak })) {
    validateResourceSnapshot(snapshot, name);
  }
  for (const field of [
    "processCount",
    "privateMemoryBytes",
    "workingSetBytes",
    "cpuSeconds",
  ]) {
    if (peak[field] < Math.max(before[field], after[field])) {
      throw new Error(`peak ${field} is lower than an endpoint snapshot`);
    }
  }
  return {
    before: structuredClone(before),
    after: structuredClone(after),
    peak: structuredClone(peak),
    delta: {
      processCount: after.processCount - before.processCount,
      privateMemoryBytes: after.privateMemoryBytes - before.privateMemoryBytes,
      workingSetBytes: after.workingSetBytes - before.workingSetBytes,
    },
  };
}

export function memoryClassBytes(memoryBytes = totalmem()) {
  if (!Number.isFinite(memoryBytes) || memoryBytes < 128 * 1024 * 1024) {
    throw new Error("host memory must be at least 128 MiB");
  }
  const gibibyte = 1024 ** 3;
  return Math.max(gibibyte, Math.round(memoryBytes / gibibyte) * gibibyte);
}

export function normalizedCpuPercent({
  beforeCpuSeconds,
  afterCpuSeconds,
  elapsedSeconds,
  logicalCores,
}) {
  for (const [name, value] of Object.entries({
    beforeCpuSeconds,
    afterCpuSeconds,
    elapsedSeconds,
    logicalCores,
  })) {
    if (!Number.isFinite(value) || value < 0) {
      throw new Error(`${name} must be finite and non-negative`);
    }
  }
  if (
    elapsedSeconds === 0 ||
    !Number.isInteger(logicalCores) ||
    logicalCores < 1
  ) {
    throw new Error("CPU normalization needs positive elapsed time and cores");
  }
  if (afterCpuSeconds < beforeCpuSeconds) {
    throw new Error("process-tree CPU time moved backwards");
  }
  return rounded(
    ((afterCpuSeconds - beforeCpuSeconds) / (elapsedSeconds * logicalCores)) *
      100,
  );
}

export function createProcessCpuTracker() {
  return { cumulativeSeconds: 0, initialized: false, previous: new Map() };
}

export function trackProcessCpuSeconds(tracker, samples) {
  if (!tracker || !(tracker.previous instanceof Map)) {
    throw new Error("process-tree CPU tracker is invalid");
  }
  if (!Array.isArray(samples) || samples.length === 0) {
    throw new Error("process-tree CPU samples must be a non-empty array");
  }
  const current = new Map();
  for (const sample of samples) {
    if (!sample || typeof sample.identity !== "string" || !sample.identity) {
      throw new Error("process-tree CPU identity is invalid");
    }
    if (!Number.isFinite(sample.cpuSeconds) || sample.cpuSeconds < 0) {
      throw new Error(
        "process-tree CPU seconds must be finite and non-negative",
      );
    }
    if (current.has(sample.identity)) {
      throw new Error(`duplicate process-tree CPU identity ${sample.identity}`);
    }
    current.set(sample.identity, sample.cpuSeconds);
    if (!tracker.initialized) continue;
    const previous = tracker.previous.get(sample.identity);
    tracker.cumulativeSeconds +=
      previous === undefined
        ? sample.cpuSeconds
        : Math.max(0, sample.cpuSeconds - previous);
  }
  tracker.previous = current;
  tracker.initialized = true;
  return rounded(tracker.cumulativeSeconds);
}

export function commonHostMetadata(overrides = {}) {
  const processors = cpus();
  const memoryBytes = Math.trunc(totalmem());
  return {
    cpuModel: processors[0]?.model?.trim() || "unknown",
    logicalCores: processors.length,
    memoryBytes,
    memoryClassBytes: memoryClassBytes(memoryBytes),
    ...overrides,
  };
}

export function createPerformanceReport({
  status = "passed",
  benchmark,
  host,
  fixture,
  sampling,
  metricSamples,
  resources,
  assertions = [],
  error,
}) {
  const metrics = Object.fromEntries(
    Object.entries(metricSamples).map(([name, metric]) => [
      name,
      summarizeSamples(metric.samples, metric.unit),
    ]),
  );
  const report = {
    schemaVersion: REPORT_SCHEMA_VERSION,
    status,
    benchmark: structuredClone(benchmark),
    host: structuredClone(host),
    fixture: structuredClone(fixture),
    sampling: structuredClone(sampling),
    metrics,
    resources: structuredClone(resources),
    gate: {
      status: "not-evaluated",
      baselineCompatible: false,
      baselineCommit: benchmark.baselineCommit,
      violations: [],
      comparisons: [],
    },
    assertions: [...assertions],
    ...(error ? { error: String(error) } : {}),
  };
  validatePerformanceReport(report);
  return report;
}

export function validatePerformanceReport(report) {
  if (!validateSchema(report)) {
    const details = validateSchema.errors
      .map((error) => `${error.instancePath || "/"} ${error.message}`)
      .join("; ");
    throw new Error(`invalid performance report schema: ${details}`);
  }
  const requiredMetrics = REQUIRED_METRICS[report.benchmark.suite];
  for (const name of requiredMetrics) {
    if (!Object.hasOwn(report.metrics, name)) {
      throw new Error(`performance report is missing metric ${name}`);
    }
  }
  for (const [name, summary] of Object.entries(report.metrics)) {
    const expected = summarizeSamples(summary.samples, summary.unit);
    if (JSON.stringify(expected) !== JSON.stringify(summary)) {
      throw new Error(`metric ${name} summary does not match its raw samples`);
    }
    const expectedCount = expectedSampleCount(report, name);
    if (expectedCount !== undefined && summary.sampleCount !== expectedCount) {
      throw new Error(
        `metric ${name} has ${summary.sampleCount} samples; expected ${expectedCount}`,
      );
    }
  }
  if (report.status === "failed" && !report.error) {
    throw new Error("a failed performance report must include an error");
  }
  if (report.status === "passed" && report.error) {
    throw new Error("a passed performance report cannot include an error");
  }
  for (const [metric, resource] of Object.entries({
    processCountGrowth: "processCount",
    privateMemoryGrowthBytes: "privateMemoryBytes",
    workingSetGrowthBytes: "workingSetBytes",
  })) {
    const expectedGrowth = Math.max(0, report.resources.delta[resource]);
    if (report.metrics[metric].samples[0] !== expectedGrowth) {
      throw new Error(
        `metric ${metric} does not match resources.delta.${resource}`,
      );
    }
  }
  if (
    report.benchmark.suite === "release-webview" &&
    (!report.fixture.workspaceTiers ||
      report.fixture.catalogModes.length !== 2 ||
      report.fixture.environmentModes.length !== 3)
  ) {
    throw new Error(
      "release WebView reports require realistic workspace, catalog, and environment fixtures",
    );
  }
  if (
    report.benchmark.suite === "installed-bundle" &&
    (report.fixture.workspaceTiers !== null ||
      report.fixture.catalogModes.length !== 0 ||
      report.fixture.environmentModes.length !== 0)
  ) {
    throw new Error(
      "installed bundle reports cannot claim WebView IPC fixtures",
    );
  }
  return report;
}

export function validatePerformancePolicy(policy) {
  if (!validatePolicySchema(policy)) {
    const details = validatePolicySchema.errors
      .map((error) => `${error.instancePath || "/"} ${error.message}`)
      .join("; ");
    throw new Error(`invalid performance budget policy: ${details}`);
  }
  return policy;
}

export function assertCompatibleReports(candidate, baseline, policy) {
  validatePerformanceReport(candidate);
  validatePerformanceReport(baseline);
  validatePerformancePolicy(policy);
  if (candidate.benchmark.baselineCommit !== baseline.benchmark.commit) {
    throw new Error(
      `candidate baseline commit ${candidate.benchmark.baselineCommit} does not match report commit ${baseline.benchmark.commit}`,
    );
  }
  if (baseline.status !== "passed") {
    throw new Error("the baseline report did not pass");
  }
  if (!Array.isArray(policy.compatibilityFields)) {
    throw new Error("performance policy has no compatibilityFields array");
  }
  const mismatches = [];
  for (const field of policy.compatibilityFields) {
    const candidateValue = valueAtPath(candidate, field);
    const baselineValue = valueAtPath(baseline, field);
    if (candidateValue === undefined || baselineValue === undefined) {
      mismatches.push(`${field} is missing`);
    } else if (stableJson(candidateValue) !== stableJson(baselineValue)) {
      mismatches.push(
        `${field}: candidate=${stableJson(candidateValue)}, baseline=${stableJson(baselineValue)}`,
      );
    }
  }
  if (mismatches.length > 0) {
    throw new Error(
      `incompatible performance reports: ${mismatches.join("; ")}`,
    );
  }
  return true;
}

export function evaluatePerformance(candidate, baseline, policy) {
  assertCompatibleReports(candidate, baseline, policy);
  const suitePolicy =
    policy.platforms?.[candidate.benchmark.platform]?.suites?.[
      candidate.benchmark.suite
    ];
  if (!suitePolicy || typeof suitePolicy.metrics !== "object") {
    throw new Error(
      `missing ${candidate.benchmark.platform}/${candidate.benchmark.suite} metric budgets in performance policy`,
    );
  }
  const violations = [];
  const comparisons = [];
  for (const [metric, budget] of Object.entries(suitePolicy.metrics)) {
    const statistic = budget.statistic;
    if (!["p50", "p95", "max"].includes(statistic)) {
      throw new Error(`metric ${metric} has an unsupported statistic`);
    }
    const actualSummary = candidate.metrics[metric];
    const baselineSummary = baseline.metrics[metric];
    if (!actualSummary || !baselineSummary) {
      violations.push({
        metric,
        statistic,
        kind: "missing",
        actual: actualSummary ? actualSummary[statistic] : -1,
        limit: budget.absoluteMax,
      });
      continue;
    }
    if (actualSummary.unit !== baselineSummary.unit) {
      throw new Error(`metric ${metric} changed unit between reports`);
    }
    const actual = actualSummary[statistic];
    const baselineValue = baselineSummary[statistic];
    const relativeChangePercent = relativeChange(actual, baselineValue);
    const relativeNoiseFloor =
      suitePolicy.relativeNoiseFloor[actualSummary.unit];
    const relativeLimit = rounded(
      baselineValue +
        Math.max(
          (baselineValue * budget.relativeMaxPercent) / 100,
          relativeNoiseFloor,
        ),
    );
    const absolutePassed = actual <= budget.absoluteMax;
    const relativePassed = actual <= relativeLimit;
    const passed = absolutePassed && relativePassed;
    comparisons.push({
      metric,
      statistic,
      actual,
      baseline: baselineValue,
      relativeChangePercent: rounded(relativeChangePercent),
      absoluteLimit: budget.absoluteMax,
      relativeLimitPercent: budget.relativeMaxPercent,
      relativeNoiseFloor,
      relativeLimit,
      passed,
    });
    if (!absolutePassed) {
      violations.push({
        metric,
        statistic,
        kind: "absolute",
        actual,
        limit: budget.absoluteMax,
        baseline: baselineValue,
        relativeChangePercent: rounded(relativeChangePercent),
      });
    }
    if (!relativePassed) {
      violations.push({
        metric,
        statistic,
        kind: "relative",
        actual,
        limit: relativeLimit,
        baseline: baselineValue,
        relativeChangePercent: rounded(relativeChangePercent),
      });
    }
  }
  candidate.gate = {
    status: violations.length === 0 ? "passed" : "failed",
    baselineCompatible: true,
    baselineCommit: baseline.benchmark.commit,
    violations,
    comparisons,
  };
  if (violations.length > 0) {
    candidate.status = "failed";
    candidate.error = `performance gate failed with ${violations.length} violation(s)`;
  }
  validatePerformanceReport(candidate);
  return candidate.gate;
}

export function samplingPolicy(overrides = {}) {
  return {
    warmupCount: 1,
    sampleCount: 7,
    percentileMethod: "nearest-rank",
    dispersion: "median-absolute-deviation-and-iqr",
    outlierPolicy: "tukey-iqr-report-only-never-discarded",
    rerunPolicy: "one-infrastructure-rerun-performance-failure-preserved",
    idleWindowSeconds: 300,
    idleSampleSeconds: 5,
    environmentSlowDelayMs: 400,
    environmentClientTimeoutMs: 750,
    cpuNormalization:
      "process-tree-cpu-seconds/(wall-seconds*logical-cores)*100",
    processTreeScope: "root-and-live-descendants-at-each-sample",
    ...overrides,
  };
}

export function renderPerformanceMarkdown(report) {
  validatePerformanceReport(report);
  const lines = [
    `## ${report.benchmark.platform} ${report.benchmark.suite} performance`,
    "",
    `Commit \`${report.benchmark.commit.slice(0, 12)}\` vs baseline \`${report.benchmark.baselineCommit.slice(0, 12)}\`; gate: **${report.gate.status}**.`,
    "",
    "| Metric | n | Candidate p50 | Candidate p95 | Gate value | Baseline | Change | Limits | Result |",
    "| --- | ---: | ---: | ---: | --- | ---: | ---: | --- | --- |",
  ];
  for (const comparison of report.gate.comparisons) {
    const summary = report.metrics[comparison.metric];
    const unit = summary.unit;
    lines.push(
      `| ${comparison.metric} | ${summary.sampleCount} | ${formatMetric(summary.p50, unit)} | ${formatMetric(summary.p95, unit)} | ${comparison.statistic}: ${formatMetric(comparison.actual, unit)} | ${formatMetric(comparison.baseline, unit)} | ${comparison.relativeChangePercent.toFixed(2)}% | abs ${formatMetric(comparison.absoluteLimit, unit)}; rel ${formatMetric(comparison.relativeLimit, unit)} (${comparison.relativeLimitPercent}% / floor ${formatMetric(comparison.relativeNoiseFloor, unit)}) | ${comparison.passed ? "pass" : "fail"} |`,
    );
  }
  lines.push(
    "",
    `Samples: ${report.sampling.sampleCount} after ${report.sampling.warmupCount} warm-up; percentiles use nearest-rank. Tukey-IQR outliers are reported and retained. Idle window: ${report.sampling.idleWindowSeconds}s in ${report.sampling.idleSampleSeconds}s samples.`,
  );
  if (report.gate.violations.length > 0) {
    lines.push("", "Violations:");
    for (const violation of report.gate.violations) {
      lines.push(
        `- ${violation.metric} ${violation.statistic}: ${violation.kind} limit exceeded (${violation.actual} > ${violation.limit})`,
      );
    }
  }
  return `${lines.join("\n")}\n`;
}

function checkedSamples(samples) {
  if (!Array.isArray(samples) || samples.length === 0) {
    throw new Error("metric samples must be a non-empty array");
  }
  return samples.map((value, index) => {
    if (!Number.isFinite(value) || value < 0) {
      throw new Error(
        `metric sample ${index} must be a finite non-negative number`,
      );
    }
    return value;
  });
}

function expectedSampleCount(report, metric) {
  if (
    metric.endsWith("GrowthBytes") ||
    metric === "processCountGrowth" ||
    metric === "installMs" ||
    metric === "uninstallMs"
  ) {
    return 1;
  }
  if (
    [
      "idleCpuPercent",
      "privateMemoryBytes",
      "workingSetBytes",
      "processCount",
    ].includes(metric)
  ) {
    return Math.ceil(
      report.sampling.idleWindowSeconds / report.sampling.idleSampleSeconds,
    );
  }
  if (metric === "backgroundPollingCpuPercent") {
    return Math.min(
      12,
      Math.ceil(
        report.sampling.idleWindowSeconds / report.sampling.idleSampleSeconds,
      ),
    );
  }
  if (REQUIRED_METRICS[report.benchmark.suite].includes(metric)) {
    return report.sampling.sampleCount;
  }
  return undefined;
}

function validateResourceSnapshot(snapshot, name) {
  if (!snapshot || typeof snapshot !== "object") {
    throw new Error(`${name} resource snapshot is missing`);
  }
  for (const field of [
    "processCount",
    "privateMemoryBytes",
    "workingSetBytes",
    "cpuSeconds",
  ]) {
    const value = snapshot[field];
    if (!Number.isFinite(value) || value < 0) {
      throw new Error(`${name}.${field} must be finite and non-negative`);
    }
  }
  if (!Number.isInteger(snapshot.processCount) || snapshot.processCount < 1) {
    throw new Error(`${name}.processCount must be a positive integer`);
  }
  for (const field of ["privateMemoryBytes", "workingSetBytes"]) {
    if (!Number.isInteger(snapshot[field])) {
      throw new Error(`${name}.${field} must be an integer`);
    }
  }
}

function relativeChange(actual, baseline) {
  if (baseline === 0) return actual === 0 ? 0 : 1_000_000_000;
  return ((actual - baseline) / baseline) * 100;
}

function valueAtPath(value, path) {
  return path.split(".").reduce((current, key) => current?.[key], value);
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function rounded(value) {
  const factor = 10 ** ROUNDING_DIGITS;
  return Math.round(value * factor) / factor;
}

function formatMetric(value, unit) {
  if (unit === "bytes") return `${(value / 1024 ** 2).toFixed(2)} MiB`;
  if (unit === "percent") return `${value.toFixed(2)}%`;
  if (unit === "ms") return `${value.toFixed(2)} ms`;
  return value.toFixed(2);
}
