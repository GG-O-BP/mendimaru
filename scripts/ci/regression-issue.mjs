import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import {
  readAttributionPaths,
  unchangedViolationInputs,
} from "./regression-attribution.mjs";

import {
  isGateReportName,
  missingGateVerdicts,
  readDirectoryNames,
  readGateManifests,
} from "./gate-reports.mjs";

// Detection for the idle phase and the installed-bundle suite now happens
// after the merge (issue #176 split SLA). Deferred detection is only
// defensible if the failure it finds cannot be ignored, so a post-merge gate
// failure has to leave a durable, labelled artifact behind on its own.
//
// The label the merge hold reads. Removing it, or closing the issue, is the
// documented way a human clears the hold.
export const regressionLabel = "ci:perf-regression";

// Applied only when a specific merged commit is implicated and the gate
// actually reported a budget violation. A scheduled run is not attributable to
// one merge, and an infrastructure failure is not a reason to revert product
// code, so neither earns this label.
export const revertCandidateLabel = "revert-candidate";

// GitHub rejects issue bodies past 65536 characters. Truncating deterministically
// keeps a pathological run from failing the very step that records the failure.
const maxBodyLength = 60000;

const jobsWithoutMetricGates = new Set(["cancelled", "skipped", "success"]);

// Mirrors the `gate.status` enum in schemas/performance-report.schema.json.
// `not-evaluated` is the value every report is born with, so a report the gate
// never reached is ordinary rather than broken. A unit test asserts this stays
// equal to the schema enum, because silently rejecting a legitimate status
// would file the report as unreadable and hide a real measurement.
export const gateStatuses = ["not-evaluated", "passed", "failed"];

export function regressionMarker(commit) {
  const sha = requireCommit(commit);
  return `<!-- post-merge-performance-regression:${sha} -->`;
}

// Only a hard failure is a regression signal. A cancelled or skipped gate says
// nothing was measured, and treating it as a failure would file issues every
// time a run is superseded by the concurrency group.
export function failedGateJobs(jobResults) {
  if (!jobResults || typeof jobResults !== "object") return [];
  return Object.entries(jobResults)
    .filter(([, result]) => !jobsWithoutMetricGates.has(String(result ?? "")))
    .filter(([, result]) => String(result ?? "") === "failure")
    .map(([job]) => job)
    .sort();
}

export function shouldFileRegressionIssue(jobResults) {
  return failedGateJobs(jobResults).length > 0;
}

// Reads the evaluated candidate reports the gate jobs upload. The gate already
// wrote its own verdict into each report, so this reports that verdict rather
// than re-deriving it from the budgets. Re-deriving would let the issue and the
// gate disagree after a budget change.
//
// Issue #192: this used to read every `*.json` under the directory and to say
// nothing about reports that were absent. Both halves were wrong at once, and
// they were wrong in opposite directions -- raw measurement dumps were shouted
// about while a gate verdict that never existed was silent. Reads are now
// restricted to the closed gate-report whitelist, and the gate manifests are
// reconciled so an absent verdict becomes an explicit, higher-severity finding.
export function readEvaluatedReports(directory, expectedCommit = "") {
  const summaries = [];
  const problems = [];
  // Degrade rather than throw. A bad commit must not abort the run that is
  // supposed to record the failure, so an unusable filter is downgraded to a
  // recorded problem and every report is kept.
  let normalizedExpectedCommit = "";
  if (expectedCommit) {
    try {
      normalizedExpectedCommit = requireCommit(expectedCommit);
    } catch (error) {
      problems.push(`commit filter disabled: ${message(error)}`);
    }
  }
  let entries;
  try {
    entries = collectGateReportFiles(directory);
  } catch (error) {
    return {
      summaries,
      problems: [`unreadable report directory: ${message(error)}`],
      missing: [
        {
          scope: "all",
          kind: "no-verdict",
          detail:
            "게이트 리포트 디렉터리를 읽지 못해 어떤 판정이 존재하는지 확인할 수 없다",
        },
      ],
    };
  }
  const { manifests, problems: manifestProblems } =
    readGateManifests(directory);
  problems.push(...manifestProblems);
  const missing = missingGateVerdicts({
    manifests,
    presentFiles: readDirectoryNames(directory),
  });
  for (const file of entries) {
    let parsed;
    try {
      parsed = JSON.parse(readFileSync(file, "utf8"));
    } catch (error) {
      problems.push(`${path.basename(file)}: unparseable (${message(error)})`);
      continue;
    }
    try {
      const summary = summarizeEvaluatedReport(parsed, path.basename(file));
      if (
        normalizedExpectedCommit &&
        summary.commit.toLowerCase() !== normalizedExpectedCommit
      ) {
        problems.push(
          `${path.basename(file)}: commit ${JSON.stringify(summary.commit)} does not match ${normalizedExpectedCommit}`,
        );
        continue;
      }
      summaries.push(summary);
    } catch (error) {
      problems.push(`${path.basename(file)}: unusable (${message(error)})`);
    }
  }
  summaries.sort((left, right) => left.name.localeCompare(right.name));
  problems.sort();
  return { summaries, problems, missing };
}

export function summarizeEvaluatedReport(report, name = "report.json") {
  if (!report || typeof report !== "object") {
    throw new Error("report is not an object");
  }
  const benchmark = report.benchmark;
  if (!benchmark || typeof benchmark !== "object") {
    throw new Error("report has no benchmark block");
  }
  // An ungated report is legitimate: a report uploaded by a measure job that
  // the gate never evaluated has no verdict to quote.
  const gate =
    report.gate && typeof report.gate === "object" ? report.gate : null;
  const violations = Array.isArray(gate?.violations) ? gate.violations : [];
  if (gate && !gateStatuses.includes(String(gate.status))) {
    throw new Error(
      `report has unsupported gate status ${JSON.stringify(gate.status)}`,
    );
  }
  // Only `failed` may carry violations. A `passed` or `not-evaluated` report
  // that lists them contradicts itself, and quoting it would attribute
  // violations the gate never actually returned.
  if (gate && gate.status !== "failed" && violations.length > 0) {
    throw new Error(`${String(gate.status)} report contains gate violations`);
  }
  return {
    name,
    platform: String(benchmark.platform ?? "unknown"),
    suite: String(benchmark.suite ?? "unknown"),
    commit: String(benchmark.commit ?? ""),
    baselineCommit: String(benchmark.baselineCommit ?? ""),
    status: gate ? String(gate.status ?? "unknown") : "not-evaluated",
    violations: violations.map((violation) => ({
      metric: String(violation.metric ?? "unknown"),
      statistic: String(violation.statistic ?? ""),
      kind: String(violation.kind ?? ""),
      actual: violation.actual,
      limit: violation.limit,
      baseline: violation.baseline,
      relativeChangePercent: violation.relativeChangePercent,
    })),
  };
}

export function violationCount(summaries) {
  return summaries.reduce(
    (total, summary) => total + summary.violations.length,
    0,
  );
}

export function buildRegressionIssue(input) {
  const commit = requireCommit(input.commit);
  const summaries = input.summaries ?? [];
  const problems = input.problems ?? [];
  const missing = input.missing ?? [];
  const failedJobs = input.failedJobs ?? [];
  const eventName = String(input.eventName ?? "push");
  const violations = violationCount(summaries);
  const unchangedInputs = unchangedViolationInputs(input);
  const attributable =
    eventName === "push" && violations > 0 && !unchangedInputs;

  const short = commit.slice(0, 12);
  const title =
    violations > 0
      ? `ci(perf): 병합 후 성능 게이트 실패 — \`${short}\``
      : `ci(perf): 병합 후 성능 런 실패(예산 위반 없음) — \`${short}\``;

  const labels = [regressionLabel];
  if (attributable) labels.push(revertCandidateLabel);

  const lines = [regressionMarker(commit), ""];

  if (violations > 0) {
    lines.push(
      `\`${short}\` 에서 병합 후 성능 게이트가 **${violations}건의 예산 위반**으로 실패했다.`,
    );
  } else {
    // Saying "budget regression" here without an evaluated violation would
    // send a human to revert product code without evidence. The failure may be
    // infrastructure, or it may be a product crash before a report existed;
    // the issue records it for triage without choosing between those causes.
    lines.push(
      missing.length > 0
        ? `\`${short}\` 의 병합 후 성능 런이 실패했고 **예산 위반은 보고되지 않았다.** 다만 아래 범위는 **애초에 판정이 없다** — 위반 0건은 "회귀 없음"이 아니라 "확인되지 않음"이다. 특정 병합 커밋의 예산 회귀로 귀속할 근거가 없어 revert 후보로는 표시하지 않았다.`
        : `\`${short}\` 의 병합 후 성능 런이 실패했지만 **예산 위반은 보고되지 않았다.** 인프라·아티팩트 오류인지, 리포트 생성 전 제품 실행 실패인지 현재 증거만으로 귀속할 수 없어 revert 후보로 표시하지 않았다.`,
    );
  }
  lines.push("");
  lines.push(`- 커밋: \`${commit}\``);
  if (input.baselineCommit)
    lines.push(`- 기준(baseline): \`${input.baselineCommit}\``);
  lines.push(`- 이벤트: \`${eventName}\``);
  if (input.runUrl) lines.push(`- 런: ${input.runUrl}`);
  lines.push(
    `- 실패한 job: ${failedJobs.length > 0 ? failedJobs.map((job) => `\`${job}\``).join(", ") : "(보고되지 않음)"}`,
  );
  lines.push("");

  if (unchangedInputs) {
    lines.push("## 병합 커밋 귀속");
    lines.push("");
    lines.push(
      "실패한 suite의 제품·하네스·예산 입력을 바꾸지 않은 변경이다. 원본 실패와 모든 예산 위반은 보존하지만 이 커밋을 되돌릴 근거로는 사용하지 않는다. 제품이 정상이라는 판정이나 측정 통과를 뜻하지 않는다.",
    );
    for (const file of input.changedPaths) lines.push(`- 변경: \`${file}\``);
    lines.push("");
  }

  // Deliberately above the violation table. A verdict that failed is a known
  // quantity; a verdict that does not exist is worse, because nothing about
  // that scope has been checked at all. Issue #192 was filed precisely because
  // a reader of #186 saw two harmless file-format complaints and never learned
  // that both WebView verdicts were missing.
  if (missing.length > 0) {
    lines.push("## ⚠️ 판정 결손 — 확인되지 않은 범위가 있다");
    lines.push("");
    lines.push(
      "아래 범위는 **게이트 판정이 존재하지 않는다.** 통과한 것이 아니라 확인되지 않은 것이므로, 위반 목록이 비어 있다는 사실을 회귀 없음의 근거로 쓸 수 없다.",
    );
    lines.push("");
    for (const entry of missing) {
      lines.push(`- \`${entry.scope}\`: ${entry.detail}`);
    }
    lines.push("");
  }

  if (violations > 0) {
    lines.push("## 위반 지표");
    lines.push("");
    lines.push("| 리포트 | 지표 | 통계 | 종류 | 실측 | 한계 | 기준 | 변화 |");
    lines.push("|---|---|---|---|---:|---:|---:|---:|");
    appendViolationRows(lines, summaries);
    lines.push("");
  }

  if (summaries.length === 0) {
    // The issue must still be filed. Losing the record because the artifacts
    // could not be read would reproduce exactly the gap this job closes.
    lines.push(
      "평가된 리포트를 읽지 못해 지표 상세를 첨부하지 못했다. 런 로그와 step summary 를 직접 확인할 것.",
    );
    lines.push("");
  }

  if (problems.length > 0) {
    lines.push("## 리포트 읽기 문제");
    lines.push("");
    for (const problem of problems) lines.push(`- ${problem}`);
    lines.push("");
  }

  if (attributable) {
    lines.push("## 이 이슈가 막고 있는 것");
    lines.push("");
    lines.push(
      `이 이슈가 \`${regressionLabel}\` 와 \`${revertCandidateLabel}\` 라벨을 달고 열려 있는 동안 \`Post-merge performance hold\` 체크가 실패한다. 해제하려면 회귀를 고치거나, 오탐으로 판단한 근거를 남기고 이슈를 닫거나 \`${revertCandidateLabel}\` 라벨을 제거한다.`,
    );
  } else {
    lines.push("## 병합 보류 여부");
    lines.push("");
    lines.push(
      `이 실패는 특정 병합 커밋의 예산 회귀로 귀속되지 않아 \`${revertCandidateLabel}\` 라벨을 붙이지 않았고, 후속 병합을 보류하지 않는다.`,
    );
  }

  return {
    title,
    body: truncate(lines.join("\n")),
    labels,
    marker: regressionMarker(commit),
  };
}

// A re-run of the same commit must not open a second issue. Matching on the
// marker rather than the title keeps dedupe working after a human edits the
// title.
export function selectExistingIssue(issues, marker) {
  if (!Array.isArray(issues)) return null;
  const open = issues.filter(
    (issue) => issue && String(issue.state ?? "open").toLowerCase() === "open",
  );
  const match = open.find(
    (issue) =>
      Number.isSafeInteger(Number(issue.number)) &&
      Number(issue.number) > 0 &&
      String(issue.body ?? "").includes(marker),
  );
  return match ?? null;
}

export function buildRerunComment(input) {
  const lines = [
    "같은 커밋에서 병합 후 성능 런이 다시 실패했다.",
    "",
    `- 이벤트: \`${String(input.eventName ?? "push")}\``,
  ];
  if (input.runUrl) lines.push(`- 런: ${input.runUrl}`);
  const summaries = input.summaries ?? [];
  const missing = input.missing ?? [];
  const violations = violationCount(summaries);
  lines.push(`- 위반 지표 수: ${violations}`);
  // The same reasoning as the issue body: a re-run that silently drops the
  // missing-verdict list would let a reader conclude "no violations" from a
  // run where nothing was actually checked.
  if (missing.length > 0) {
    lines.push("");
    lines.push("**판정 결손 (위반 0건을 회귀 없음으로 읽으면 안 되는 이유):**");
    for (const entry of missing) {
      lines.push(`- \`${entry.scope}\`: ${entry.detail}`);
    }
  }
  if (violations > 0) {
    lines.push("");
    lines.push("| 리포트 | 지표 | 통계 | 종류 | 실측 | 한계 | 기준 | 변화 |");
    lines.push("|---|---|---|---|---:|---:|---:|---:|");
    appendViolationRows(lines, summaries);
    lines.push("");
    lines.push(
      input.eventName !== "schedule" &&
        input.eventName !== "workflow_dispatch" &&
        !unchangedViolationInputs(input)
        ? `이번 재실행은 예산 위반을 보고했으므로 기존 이슈에도 \`${revertCandidateLabel}\` 라벨을 보장한다.`
        : "원본 예산 위반은 보존한다. 이 실행을 특정 병합 커밋의 회귀로 귀속하지 않아 revert 후보 라벨을 새로 추가하지 않는다.",
    );
  }
  return truncate(lines.join("\n"));
}

// Whitelist, not blacklist. `candidate-msi.raw.json` is excluded because it is
// not a gate report, not because `.raw.json` is special-cased; the next raw
// dump with a different suffix is excluded for the same reason.
function collectGateReportFiles(directory) {
  if (!directory) throw new Error("no report directory given");
  const stats = statSync(directory);
  if (!stats.isDirectory()) throw new Error(`${directory} is not a directory`);
  const found = [];
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && isGateReportName(entry.name)) found.push(full);
    }
  };
  walk(directory);
  return found.sort();
}

function requireCommit(commit) {
  const sha = String(commit ?? "").trim();
  // Without a commit there is no dedupe key, and a run that cannot dedupe would
  // open a fresh issue on every re-run.
  if (!/^[0-9a-f]{7,40}$/i.test(sha)) {
    throw new Error(`invalid commit sha: ${JSON.stringify(commit)}`);
  }
  return sha.toLowerCase();
}

function truncate(body) {
  if (body.length <= maxBodyLength) return body;
  const notice =
    "\n\n_(본문이 너무 길어 잘렸다. 전체 내용은 런 아티팩트를 확인할 것.)_";
  return `${body.slice(0, maxBodyLength - notice.length)}${notice}`;
}

function format(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return "-";
  return String(Math.round(value * 100) / 100);
}

function formatPercent(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return "-";
  const rounded = Math.round(value * 100) / 100;
  return `${rounded > 0 ? "+" : ""}${rounded}%`;
}

function message(error) {
  return error instanceof Error ? error.message : String(error);
}

// CLI. Two modes, both network-free: the workflow keeps every GitHub API call
// in visible `gh` glue, and every decision rule stays unit-testable here.
//
//   (no args)          build the issue decision as JSON on stdout
//   --select-existing  print the open issue number that already covers this
//                      commit, or nothing
const invokedDirectly =
  process.argv[1] && process.argv[1].endsWith("regression-issue.mjs");
if (invokedDirectly) {
  if (process.argv.includes("--select-existing")) {
    const decision = JSON.parse(
      readFileSync(requireEnv("DECISION_FILE"), "utf8"),
    );
    const issues = JSON.parse(
      readFileSync(requireEnv("OPEN_ISSUES_FILE"), "utf8"),
    );
    const existing = selectExistingIssue(issues, decision.marker);
    process.stdout.write(existing ? String(existing.number) : "");
  } else {
    const jobResults = JSON.parse(process.env.GATE_JOB_RESULTS ?? "{}");
    const failedJobs = failedGateJobs(jobResults);
    if (failedJobs.length === 0) {
      process.stdout.write(
        JSON.stringify({ file: false, reason: "no failed gate job" }),
      );
    } else {
      const { summaries, problems, missing } = readEvaluatedReports(
        process.env.REPORT_DIRECTORY ?? "reports",
        process.env.REGRESSION_COMMIT,
      );
      let changedPaths;
      try {
        changedPaths = readAttributionPaths(
          process.env.REGRESSION_BASELINE,
          process.env.REGRESSION_COMMIT,
        );
      } catch (error) {
        problems.push(`attribution diff unavailable: ${message(error)}`);
      }
      const attribution = {
        commit: process.env.REGRESSION_COMMIT,
        baselineCommit: process.env.REGRESSION_BASELINE,
        changedPaths,
        problems,
      };
      const issue = buildRegressionIssue({
        ...attribution,
        commit: process.env.REGRESSION_COMMIT,
        baselineCommit: process.env.REGRESSION_BASELINE,
        eventName: process.env.GITHUB_EVENT_NAME,
        runUrl: process.env.REGRESSION_RUN_URL,
        failedJobs,
        summaries,
        problems,
        missing,
      });
      process.stdout.write(
        JSON.stringify({
          file: true,
          ...issue,
          comment: buildRerunComment({
            ...attribution,
            eventName: process.env.GITHUB_EVENT_NAME,
            runUrl: process.env.REGRESSION_RUN_URL,
            summaries,
            missing,
          }),
        }),
      );
    }
  }
}

function requireEnv(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} must be set`);
  return value;
}

function appendViolationRows(lines, summaries) {
  for (const summary of summaries) {
    for (const violation of summary.violations) {
      lines.push(
        `| ${summary.platform}/${summary.suite} | \`${violation.metric}\` | ${violation.statistic} | ${violation.kind} | ${format(violation.actual)} | ${format(violation.limit)} | ${format(violation.baseline)} | ${formatPercent(violation.relativeChangePercent)} |`,
      );
    }
  }
}
