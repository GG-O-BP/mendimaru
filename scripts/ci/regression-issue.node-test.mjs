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
  gateStatuses,
  readEvaluatedReports,
  regressionLabel,
  regressionMarker,
  revertCandidateLabel,
  selectExistingIssue,
  shouldFileRegressionIssue,
  summarizeEvaluatedReport,
  violationCount,
} from "./regression-issue.mjs";
import { reconcileGateInputs } from "./gate-reports.mjs";

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

test("#220 preserves both NSIS violations and identifies the installer", () => {
  const fixture = JSON.parse(
    readFileSync(new URL("./fixtures/issue-220.json", import.meta.url)),
  );
  const summary = summarizeEvaluatedReport(
    fixture.report,
    "candidate-nsis.json",
  );
  assert.equal(summary.packageKind, "nsis");
  assert.equal(summary.violations.length, 2);
  assert.equal(summary.status, "failed");
  const input = {
    commit: fixture.commit,
    baselineCommit: fixture.baselineCommit,
    changedPaths: fixture.changedPaths,
    eventName: "push",
    summaries: [summary],
  };
  const issue = buildRegressionIssue(input);
  assert.deepEqual(issue.labels, [regressionLabel]);
  for (const body of [issue.body, buildRerunComment(input)]) {
    assert.match(body, /windows\/installed-bundle\/nsis/);
    assert.match(body, /workingSetBytes/);
    assert.match(body, /processCount/);
    assert.doesNotMatch(body, /installed-bundle\/msi/);
  }
  assert.deepEqual(
    fixture.installerSha256.baseline,
    fixture.installerSha256.candidate,
  );
  const both = buildRegressionIssue({
    ...input,
    summaries: [summary, { ...summary, packageKind: "msi" }],
  });
  assert.match(both.body, /installed-bundle\/msi/);
  assert.match(both.body, /installed-bundle\/nsis/);
  assert.equal(
    summarizeEvaluatedReport(evaluatedReport()).packageKind,
    "unknown",
  );
});

test("both installer gates run after input validation even when the other gate fails", () => {
  const workflow = readFileSync(
    path.join(repository, ".github/workflows/release-performance.yml"),
    "utf8",
  );
  const job = workflow
    .split("\n  installed-bundle-gate:")[1]
    .split("\n  post-merge-regression-report:")[0];
  assert.match(job, /id: bundle-inputs/);
  for (const kind of ["MSI", "NSIS"]) {
    const step = job
      .split(`- name: Gate ${kind} against current main`)[1]
      .split("\n      - name:")[0];
    assert.match(step, /!cancelled\(\)/);
    assert.match(step, /steps\.relevance\.outputs\.relevant == 'true'/);
    assert.match(step, /steps\.bundle-inputs\.outcome == 'success'/);
    assert.doesNotMatch(step, /continue-on-error|\|\| true/);
    assert.match(
      step,
      new RegExp(`reports/candidate-${kind.toLowerCase()}\\.json`),
    );
  }
});

test("a report the gate never evaluated is marked, not invented", () => {
  const summary = summarizeEvaluatedReport(
    evaluatedReport({ gate: undefined }),
  );
  assert.equal(summary.status, "not-evaluated");
  assert.deepEqual(summary.violations, []);
});

// The shape above is defensive; this is the shape the harness actually writes.
// createPerformanceReport() stamps `gate.status: "not-evaluated"` on every
// report, so the idle leg uploads one whenever the latency gate fails first.
// Rejecting it would file a perfectly good measurement as unreadable.
test("the real un-evaluated gate block is ordinary, not a problem", () => {
  const summary = summarizeEvaluatedReport(
    evaluatedReport({
      gate: {
        status: "not-evaluated",
        baselineCompatible: false,
        violations: [],
        comparisons: [],
      },
    }),
  );
  assert.equal(summary.status, "not-evaluated");
  assert.deepEqual(summary.violations, []);
});

test("the accepted gate statuses stay equal to the report schema enum", () => {
  const schema = JSON.parse(
    readFileSync(
      path.join(repository, "schemas", "performance-report.schema.json"),
      "utf8",
    ),
  );
  const enumerated = resolve(schema, schema.properties.gate)?.properties?.status
    ?.enum;
  assert.ok(enumerated, "schema must declare a gate.status enum");
  assert.deepEqual([...gateStatuses].sort(), [...enumerated].sort());
});

// The report schema keeps `gate` behind a local `$ref`, so read the pointer
// rather than a hard-coded definition name.
function resolve(schema, node) {
  if (!node?.$ref) return node;
  return node.$ref
    .replace(/^#\//, "")
    .split("/")
    .reduce((current, key) => current?.[key], schema);
}

test("a structurally unusable report is rejected rather than half-read", () => {
  assert.throws(() => summarizeEvaluatedReport(null), /not an object/);
  assert.throws(() => summarizeEvaluatedReport("nope"), /not an object/);
  assert.throws(() => summarizeEvaluatedReport({}), /no benchmark block/);
  assert.throws(
    () =>
      summarizeEvaluatedReport(
        evaluatedReport({ gate: { status: "neutral", violations: [] } }),
      ),
    /unsupported gate status/,
  );
  assert.throws(
    () =>
      summarizeEvaluatedReport(
        evaluatedReport({
          gate: { status: "passed", violations: [{ metric: "impossible" }] },
        }),
      ),
    /passed report contains gate violations/,
  );
  // Same contradiction, other legitimate status.
  assert.throws(
    () =>
      summarizeEvaluatedReport(
        evaluatedReport({
          gate: {
            status: "not-evaluated",
            violations: [{ metric: "impossible" }],
          },
        }),
      ),
    /not-evaluated report contains gate violations/,
  );
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
  assert.ok(issue.body.includes("후속 병합을 보류하지 않는다"));
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
  assert.ok(issue.body.includes("후속 병합을 보류하지 않는다"));
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

test("a malformed matching issue cannot absorb the rerun", () => {
  const marker = regressionMarker(commit);
  for (const number of [undefined, null, 0, -1, "abc"]) {
    assert.equal(
      selectExistingIssue([{ number, state: "open", body: marker }], marker),
      null,
    );
  }
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
  assert.ok(comment.includes("idleCpuPercent"));
  assert.ok(comment.includes(revertCandidateLabel));
});

test("a missing report directory is a problem, never a crash", () => {
  const result = readEvaluatedReports(path.join(scratch(), "absent"));
  assert.deepEqual(result.summaries, []);
  assert.equal(result.problems.length, 1);
});

test("a malformed report is skipped without discarding the good ones", () => {
  const directory = scratch();
  writeFileSync(
    path.join(directory, "linux-candidate-latency.json"),
    JSON.stringify(evaluatedReport()),
  );
  writeFileSync(
    path.join(directory, "linux-candidate-idle.json"),
    "{ not json",
  );
  writeFileSync(
    path.join(directory, "windows-candidate-latency.json"),
    JSON.stringify({ a: 1 }),
  );
  writeFileSync(path.join(directory, "ignored.txt"), "not a report");
  const nested = path.join(directory, "nested");
  mkdirSync(nested);
  writeFileSync(
    path.join(nested, "windows-candidate-idle.json"),
    JSON.stringify(evaluatedReport()),
  );

  const result = readEvaluatedReports(directory);
  assert.equal(result.summaries.length, 2);
  assert.equal(result.problems.length, 2);
  assert.ok(
    result.problems.some((problem) =>
      problem.includes("linux-candidate-idle.json"),
    ),
  );
  assert.ok(
    result.problems.some((problem) =>
      problem.includes("windows-candidate-latency.json"),
    ),
  );
});

test("a report for another commit is never attributed to this merge", () => {
  const directory = scratch();
  writeFileSync(
    path.join(directory, "linux-candidate-latency.json"),
    JSON.stringify(evaluatedReport()),
  );
  const expected = "abcdef1234567890abcdef1234567890abcdef12";
  const result = readEvaluatedReports(directory, expected);
  assert.deepEqual(result.summaries, []);
  assert.equal(result.problems.length, 1);
  assert.ok(result.problems[0].includes("does not match"));
});

test("an unusable commit filter degrades instead of losing the report", () => {
  // Throwing here would abort the job that exists to record the failure, so
  // the filter is dropped and the reports are still reported.
  const directory = scratch();
  writeFileSync(
    path.join(directory, "linux-candidate-latency.json"),
    JSON.stringify(evaluatedReport()),
  );
  const result = readEvaluatedReports(directory, "not-a-sha");
  assert.equal(result.summaries.length, 1);
  assert.ok(
    result.problems.some((problem) =>
      problem.includes("commit filter disabled"),
    ),
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
  assert.ok(workflow.includes("github.ref == 'refs/heads/main'"));
  for (const job of [
    "release-webview-build",
    "release-webview-measure",
    "release-webview-gate",
    "installed-bundle-build",
    "installed-bundle-measure",
    "installed-bundle-gate",
  ]) {
    assert.ok(workflow.includes(`needs.${job}.result == 'failure'`));
  }
  assert.ok(workflow.includes(`--label "${regressionLabel}"`));
  assert.ok(
    workflow.includes('gh issue edit "$existing" --add-label "$label"'),
  );
  assert.ok(workflow.includes("--limit 1000"));
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

// Issue #192, reproduced from the real `504c28f` run 35743578471. The safety
// net reported `candidate-msi.raw.json` and `candidate-nsis.raw.json` as
// "unusable" while saying nothing about the WebView verdicts that did not
// exist at all, so a reader of #186 came away worried about two harmless raw
// dumps and unaware that the gate had never run.
function bundleReport() {
  return {
    benchmark: {
      platform: "windows",
      suite: "installed-bundle",
      commit,
      baselineCommit,
    },
    gate: { status: "passed", violations: [] },
  };
}

function rawDump(kind) {
  // Shape of what `release-performance.yml` copies to `<variant>-<kind>.raw.json`.
  // No benchmark block, because it is a raw smoke dump and never was a report.
  return { kind, samples: { coldStartupMs: [1, 2, 3] } };
}

function write504c28fFixture() {
  const directory = scratch();
  for (const kind of ["msi", "nsis"]) {
    writeFileSync(
      path.join(directory, `candidate-${kind}.json`),
      JSON.stringify(bundleReport()),
    );
    writeFileSync(
      path.join(directory, `candidate-${kind}.raw.json`),
      JSON.stringify(rawDump(kind)),
    );
  }
  writeFileSync(
    path.join(directory, "gate-manifest-installed-bundle.json"),
    JSON.stringify(
      reconcileGateInputs({
        scope: "installed-bundle",
        eventName: "push",
        relevant: true,
        found: [
          "baseline-msi.json",
          "candidate-msi.json",
          "baseline-nsis.json",
          "candidate-nsis.json",
        ],
      }),
    ),
  );
  // No webview gate manifests and no webview candidate reports: the fan-in was
  // skipped, so neither platform published anything.
  return directory;
}

test("504c28f fixture: raw dumps are not false alarms any more", () => {
  const result = readEvaluatedReports(write504c28fFixture(), commit);
  assert.deepEqual(result.problems, []);
  assert.equal(result.summaries.length, 2);
  assert.ok(
    result.summaries.every((summary) => !summary.name.includes(".raw.")),
    "a raw measurement dump must never be read as a gate verdict",
  );
});

test("504c28f fixture: the absent WebView verdicts are reported as missing", () => {
  const result = readEvaluatedReports(write504c28fFixture(), commit);
  const scopes = result.missing.map((entry) => entry.scope).sort();
  assert.deepEqual(scopes, ["linux", "windows"]);
  assert.ok(
    result.missing.every((entry) => entry.kind === "no-verdict"),
    "a gate that never ran leaves no manifest, which is the strongest signal",
  );
});

test("504c28f fixture: the issue body leads with the missing verdicts", () => {
  const { summaries, problems, missing } = readEvaluatedReports(
    write504c28fFixture(),
    commit,
  );
  const issue = buildRegressionIssue({
    commit,
    baselineCommit,
    eventName: "push",
    failedJobs: ["release-webview-measure"],
    summaries,
    problems,
    missing,
  });
  assert.ok(issue.body.includes("판정 결손"));
  // Missing verdicts must outrank the report-reading problems section, which
  // is where the two false alarms used to be the only thing a reader saw.
  const missingAt = issue.body.indexOf("판정 결손");
  const problemsAt = issue.body.indexOf("리포트 읽기 문제");
  assert.ok(missingAt > 0);
  assert.ok(problemsAt === -1 || missingAt < problemsAt);
  assert.ok(!issue.body.includes("raw.json"));
});

test("an intentional relevance skip is never reported as a missing verdict", () => {
  const directory = scratch();
  for (const scope of ["linux", "windows", "installed-bundle"]) {
    writeFileSync(
      path.join(directory, `gate-manifest-${scope}.json`),
      JSON.stringify(
        reconcileGateInputs({
          scope,
          eventName: "pull_request",
          relevant: false,
          reason: "no measured-artifact paths changed",
          found: [],
        }),
      ),
    );
  }
  const result = readEvaluatedReports(directory, commit);
  assert.deepEqual(result.missing, []);
  assert.deepEqual(result.problems, []);
});

test("a gate that ran but lost one report reports that report as missing", () => {
  const directory = scratch();
  writeFileSync(
    path.join(directory, "linux-candidate-latency.json"),
    JSON.stringify(evaluatedReport()),
  );
  for (const scope of ["linux", "windows", "installed-bundle"]) {
    writeFileSync(
      path.join(directory, `gate-manifest-${scope}.json`),
      JSON.stringify(
        reconcileGateInputs({
          scope,
          eventName: "pull_request",
          relevant: scope === "linux",
          found: scope === "linux" ? ["linux-candidate-latency.json"] : [],
        }),
      ),
    );
  }
  const result = readEvaluatedReports(directory, commit);
  // The linux baseline latency input never arrived, so the gate could not
  // compare anything even though a candidate report exists.
  assert.ok(
    result.missing.some(
      (entry) =>
        entry.kind === "missing-input" &&
        entry.detail.includes("linux-baseline-latency.json"),
    ),
  );
});

test("the re-run comment also carries the missing verdicts", () => {
  const comment = buildRerunComment({
    eventName: "push",
    runUrl: "https://example.test/run/3",
    summaries: [],
    missing: [
      {
        scope: "linux",
        kind: "no-verdict",
        detail: "게이트가 판정을 남기지 않았다",
      },
    ],
  });
  assert.ok(comment.includes("판정 결손"));
  assert.ok(comment.includes("linux"));
});
