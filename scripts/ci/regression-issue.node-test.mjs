import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  buildRegressionIssue,
  buildRerunComment,
  failedGateJobs,
  readEvaluatedReports,
  regressionLabel,
  regressionMarker,
  revertCandidateLabel,
  selectExistingIssue,
  shouldFileRegressionIssue,
  summarizeEvaluatedReport,
  violationCount,
} from "./regression-issue.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

const commit = "018b5ec4eb8303e4b13e610a17fd95574d0bc4df";
const baselineCommit = "99dafc0000000000000000000000000000000000";

function evaluatedReport(overrides = {}) {
  return {
    benchmark: {
      platform: "linux",
      suite: "release-webview",
      commit,
      baselineCommit,
    },
    gate: {
      status: "failed",
      violations: [
        {
          metric: "idleCpuPercent",
          statistic: "p95",
          kind: "relative",
          actual: 3.7,
          limit: 3.3,
          baseline: 2.3,
          relativeChangePercent: 60.95,
        },
      ],
      comparisons: [],
    },
    ...overrides,
  };
}

function scratch() {
  return mkdtempSync(path.join(tmpdir(), "regression-issue-"));
}

test("the dedupe marker is derived from the failing commit", () => {
  assert.equal(
    regressionMarker(commit),
    `<!-- post-merge-performance-regression:${commit} -->`,
  );
  assert.equal(
    regressionMarker(commit.toUpperCase()),
    regressionMarker(commit),
  );
});

test("a commit that cannot key a dedupe is rejected outright", () => {
  // Without a usable key every re-run would open another issue.
  for (const bad of ["", "   ", undefined, null, "not-a-sha", "zzzz123"]) {
    assert.throws(() => regressionMarker(bad), /invalid commit sha/);
  }
});

test("only a hard gate failure counts as a regression signal", () => {
  // A cancelled or skipped gate measured nothing. Filing on those would open an
  // issue every time the concurrency group supersedes a run.
  assert.deepEqual(
    failedGateJobs({
      "release-webview-gate": "failure",
      "installed-bundle-gate": "success",
    }),
    ["release-webview-gate"],
  );
  assert.deepEqual(
    failedGateJobs({ a: "cancelled", b: "skipped", c: "success" }),
    [],
  );
  assert.deepEqual(failedGateJobs({}), []);
  assert.deepEqual(failedGateJobs(null), []);
  assert.deepEqual(failedGateJobs(undefined), []);
});

test("both gates failing are reported together and sorted", () => {
  assert.deepEqual(
    failedGateJobs({
      "release-webview-gate": "failure",
      "installed-bundle-gate": "failure",
    }),
    ["installed-bundle-gate", "release-webview-gate"],
  );
});

test("nothing is filed when no gate failed", () => {
  assert.equal(shouldFileRegressionIssue({ a: "success" }), false);
  assert.equal(shouldFileRegressionIssue({ a: "failure" }), true);
});

test("a summary quotes the gate's own violations", () => {
  const summary = summarizeEvaluatedReport(evaluatedReport(), "linux.json");
  assert.equal(summary.platform, "linux");
  assert.equal(summary.suite, "release-webview");
  assert.equal(summary.status, "failed");
  assert.equal(summary.violations.length, 1);
  assert.equal(summary.violations[0].metric, "idleCpuPercent");
  assert.equal(violationCount([summary]), 1);
});

test("a report the gate never evaluated is marked, not invented", () => {
  const summary = summarizeEvaluatedReport(
    evaluatedReport({ gate: undefined }),
  );
  assert.equal(summary.status, "not-evaluated");
  assert.deepEqual(summary.violations, []);
});

test("a structurally unusable report is rejected rather than half-read", () => {
  assert.throws(() => summarizeEvaluatedReport(null), /not an object/);
  assert.throws(() => summarizeEvaluatedReport("nope"), /not an object/);
  assert.throws(() => summarizeEvaluatedReport({}), /no benchmark block/);
});

test("a violating push issue names a revert candidate", () => {
  const issue = buildRegressionIssue({
    commit,
    baselineCommit,
    eventName: "push",
    runUrl: "https://example.test/run/1",
    failedJobs: ["release-webview-gate"],
    summaries: [summarizeEvaluatedReport(evaluatedReport())],
    problems: [],
  });
  assert.deepEqual(issue.labels, [regressionLabel, revertCandidateLabel]);
  assert.ok(issue.body.includes(regressionMarker(commit)));
  assert.ok(issue.body.includes("idleCpuPercent"));
  assert.ok(issue.body.includes("release-webview-gate"));
  assert.ok(issue.body.includes("https://example.test/run/1"));
  assert.ok(issue.title.includes(commit.slice(0, 12)));
});

test("a failure with no budget violation is not called a regression", () => {
  // Labelling an infrastructure failure as a revert candidate would send a
  // human to revert product code that measured clean.
  const issue = buildRegressionIssue({
    commit,
    eventName: "push",
    failedJobs: ["installed-bundle-gate"],
    summaries: [
      summarizeEvaluatedReport(
        evaluatedReport({ gate: { status: "passed", violations: [] } }),
      ),
    ],
    problems: [],
  });
  assert.deepEqual(issue.labels, [regressionLabel]);
  assert.ok(!issue.labels.includes(revertCandidateLabel));
  assert.ok(issue.body.includes("예산 위반은 보고되지 않았다"));
});

test("a scheduled failure is not attributed to one merge", () => {
  // The weekly run measures whatever is on main; no single commit is implicated.
  const issue = buildRegressionIssue({
    commit,
    eventName: "schedule",
    failedJobs: ["release-webview-gate"],
    summaries: [summarizeEvaluatedReport(evaluatedReport())],
  });
  assert.deepEqual(issue.labels, [regressionLabel]);
  assert.ok(issue.body.includes("`schedule`"));
});

test("the issue is still filed when no report could be read", () => {
  // Losing the record because an artifact was missing would reproduce the very
  // gap this job closes.
  const issue = buildRegressionIssue({
    commit,
    eventName: "push",
    failedJobs: ["release-webview-gate"],
    summaries: [],
    problems: ["reports: unreadable"],
  });
  assert.ok(issue.body.includes(regressionMarker(commit)));
  assert.ok(issue.body.includes("지표 상세를 첨부하지 못했다"));
  assert.ok(issue.body.includes("reports: unreadable"));
  assert.deepEqual(issue.labels, [regressionLabel]);
});

test("an empty failed-job list is still recorded honestly", () => {
  const issue = buildRegressionIssue({
    commit,
    eventName: "push",
    failedJobs: [],
    summaries: [],
  });
  assert.ok(issue.body.includes("(보고되지 않음)"));
});

test("a pathological body is truncated instead of rejected by GitHub", () => {
  const many = Array.from({ length: 4000 }, (_, index) => ({
    metric: `metric_${index}_${"x".repeat(40)}`,
    statistic: "p95",
    kind: "relative",
    actual: index,
    limit: index,
    baseline: index,
    relativeChangePercent: index,
  }));
  const issue = buildRegressionIssue({
    commit,
    eventName: "push",
    failedJobs: ["release-webview-gate"],
    summaries: [
      summarizeEvaluatedReport(
        evaluatedReport({ gate: { status: "failed", violations: many } }),
      ),
    ],
  });
  assert.ok(issue.body.length <= 60000);
  assert.ok(issue.body.includes("잘렸다"));
});

test("a non-finite metric renders as a placeholder, not NaN", () => {
  const issue = buildRegressionIssue({
    commit,
    eventName: "push",
    failedJobs: ["release-webview-gate"],
    summaries: [
      summarizeEvaluatedReport(
        evaluatedReport({
          gate: {
            status: "failed",
            violations: [
              { metric: "missingMetric", statistic: "p95", kind: "missing" },
            ],
          },
        }),
      ),
    ],
  });
  assert.ok(!issue.body.includes("NaN"));
  assert.ok(!issue.body.includes("undefined"));
});

test("a re-run finds the open issue for the same commit", () => {
  const marker = regressionMarker(commit);
  const issues = [
    { number: 10, state: "open", body: "unrelated" },
    { number: 11, state: "open", body: `intro\n${marker}\nrest` },
  ];
  assert.equal(selectExistingIssue(issues, marker).number, 11);
});

test("a closed issue never suppresses a fresh report", () => {
  const marker = regressionMarker(commit);
  assert.equal(
    selectExistingIssue(
      [{ number: 11, state: "closed", body: marker }],
      marker,
    ),
    null,
  );
  assert.equal(selectExistingIssue([], marker), null);
  assert.equal(selectExistingIssue(null, marker), null);
});

test("a different commit's issue is not mistaken for this one", () => {
  const other = regressionMarker("abcdef1234567890abcdef1234567890abcdef12");
  assert.equal(
    selectExistingIssue(
      [{ number: 11, state: "open", body: other }],
      regressionMarker(commit),
    ),
    null,
  );
});

test("the re-run comment stays short and states the violation count", () => {
  const comment = buildRerunComment({
    eventName: "push",
    runUrl: "https://example.test/run/2",
    summaries: [summarizeEvaluatedReport(evaluatedReport())],
  });
  assert.ok(comment.includes("https://example.test/run/2"));
  assert.ok(comment.includes("1"));
});

test("a missing report directory is a problem, never a crash", () => {
  const result = readEvaluatedReports(path.join(scratch(), "absent"));
  assert.deepEqual(result.summaries, []);
  assert.equal(result.problems.length, 1);
});

test("a malformed report is skipped without discarding the good ones", () => {
  const directory = scratch();
  writeFileSync(
    path.join(directory, "good.json"),
    JSON.stringify(evaluatedReport()),
  );
  writeFileSync(path.join(directory, "broken.json"), "{ not json");
  writeFileSync(
    path.join(directory, "shapeless.json"),
    JSON.stringify({ a: 1 }),
  );
  writeFileSync(path.join(directory, "ignored.txt"), "not a report");
  const nested = path.join(directory, "nested");
  mkdirSync(nested);
  writeFileSync(
    path.join(nested, "also-good.json"),
    JSON.stringify(evaluatedReport()),
  );

  const result = readEvaluatedReports(directory);
  assert.equal(result.summaries.length, 2);
  assert.equal(result.problems.length, 2);
  assert.ok(result.problems.some((problem) => problem.includes("broken.json")));
  assert.ok(
    result.problems.some((problem) => problem.includes("shapeless.json")),
  );
});

test("release-performance.yml actually wires this safety net up", () => {
  const workflow = readFileSync(
    path.join(repository, ".github", "workflows", "release-performance.yml"),
    "utf8",
  );
  assert.match(workflow, /^ {2}post-merge-regression-report:$/m);
  assert.ok(workflow.includes("scripts/ci/regression-issue.mjs"));
  assert.ok(workflow.includes("--select-existing"));
  // The job must be able to write issues, and nothing else may inherit it.
  assert.ok(workflow.includes("issues: write"));
  assert.equal(workflow.match(/issues: write/g).length, 1);
  // Must survive a failed dependency, otherwise it can never observe a failure.
  assert.ok(workflow.includes("always()"));
  assert.ok(workflow.includes(`--label "${regressionLabel}"`));
});

test("the gates publish the evaluated reports the report job reads", () => {
  const workflow = readFileSync(
    path.join(repository, ".github", "workflows", "release-performance.yml"),
    "utf8",
  );
  assert.ok(workflow.includes("webview-gate-report-"));
  assert.ok(workflow.includes("installed-bundle-gate-report"));
  assert.ok(workflow.includes('pattern: "*gate-report*"'));
});
