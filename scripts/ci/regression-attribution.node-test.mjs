import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  readAttributionPaths,
  unchangedViolationInputs,
} from "./regression-attribution.mjs";
import {
  buildRegressionIssue,
  buildRerunComment,
  summarizeEvaluatedReport,
} from "./regression-issue.mjs";

const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/issue-214.json", import.meta.url)),
);
const input = {
  commit: fixture.commit,
  baselineCommit: fixture.baselineCommit,
  changedPaths: fixture.changedPaths,
  eventName: "push",
  summaries: [summarizeEvaluatedReport(fixture.report)],
};

test("#214 preserves the failed CPU verdict without blaming an unrelated latency policy", () => {
  assert.equal(fixture.report.gate.status, "failed");
  assert.deepEqual(
    fixture.installerSha256.baseline,
    fixture.installerSha256.candidate,
  );
  const issue = buildRegressionIssue(input);
  assert.deepEqual(issue.labels, ["ci:perf-regression"]);
  assert.match(issue.body, /idleCpuPercent/);
  assert.match(issue.body, /2\.07/);
  assert.match(issue.body, /측정 통과를 뜻하지 않는다/);
  assert.doesNotMatch(buildRerunComment(input), /라벨을 보장한다/);
  assert.equal(fixture.report.metrics.idleCpuPercent.samples.length, 60);
});

test("unreadable, empty, malformed or unknown diffs retain the revert candidate", () => {
  for (const changedPaths of [
    undefined,
    [],
    [null],
    [""],
    ["../docs/readme.md"],
    ["docs/../src/App.tsx"],
    ["new-input.dat"],
    ["performance/budgets.bundle.json"],
    ["scripts/perf/performance-core.mjs"],
    ["scripts/e2e/windows-bundle-smoke.ps1"],
    ["scripts/browser-driver.mjs"],
    ["src/App.tsx"],
    ["src-tauri/tauri.conf.json"],
  ]) {
    assert.equal(unchangedViolationInputs({ ...input, changedPaths }), false);
    assert.ok(
      buildRegressionIssue({ ...input, changedPaths }).labels.includes(
        "revert-candidate",
      ),
    );
  }
});

test("missing verdicts, unreadable reports and wrong revisions cannot exonerate a merge", () => {
  for (const overrides of [
    { missing: [{ scope: "windows", detail: "missing" }] },
    { problems: ["unreadable"] },
    { commit: "a".repeat(40) },
    { baselineCommit: "b".repeat(40) },
    { summaries: [{ ...input.summaries[0], suite: "unknown" }] },
    { summaries: [] },
  ])
    assert.equal(unchangedViolationInputs({ ...input, ...overrides }), false);
});

test("a latency policy change still implicates a failing WebView metric", () => {
  const webview = { ...input.summaries[0], suite: "release-webview" };
  assert.equal(
    unchangedViolationInputs({ ...input, summaries: [webview] }),
    false,
  );
  assert.equal(
    unchangedViolationInputs({
      ...input,
      summaries: [...input.summaries, webview],
    }),
    false,
  );
});

test("the diff reader refuses option injection, short or absent revisions", () => {
  for (const sha of ["--help", "HEAD", "1234567", undefined]) {
    assert.throws(
      () => readAttributionPaths(sha, fixture.commit),
      /full baseline/,
    );
  }
});
