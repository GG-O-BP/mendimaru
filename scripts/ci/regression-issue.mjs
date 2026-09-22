import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";

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
export function readEvaluatedReports(directory) {
  const summaries = [];
  const problems = [];
  let entries;
  try {
    entries = collectJsonFiles(directory);
  } catch (error) {
    return {
      summaries,
      problems: [`unreadable report directory: ${message(error)}`],
    };
  }
  for (const file of entries) {
    let parsed;
    try {
      parsed = JSON.parse(readFileSync(file, "utf8"));
    } catch (error) {
      problems.push(`${path.basename(file)}: unparseable (${message(error)})`);
      continue;
    }
    try {
      summaries.push(summarizeEvaluatedReport(parsed, path.basename(file)));
    } catch (error) {
      problems.push(`${path.basename(file)}: unusable (${message(error)})`);
    }
  }
  summaries.sort((left, right) => left.name.localeCompare(right.name));
  problems.sort();
  return { summaries, problems };
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
  const failedJobs = input.failedJobs ?? [];
  const eventName = String(input.eventName ?? "push");
  const violations = violationCount(summaries);
  const attributable = eventName === "push" && violations > 0;

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
    // Saying "regression" here when no budget was violated would send a human
    // to revert product code over an infrastructure failure.
    lines.push(
      `\`${short}\` 의 병합 후 성능 런이 실패했지만 **예산 위반은 보고되지 않았다.** 게이트 로직 자체가 아니라 인프라·아티팩트·측정 단계의 실패일 가능성이 높다. revert 후보로 표시하지 않았다.`,
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

  if (violations > 0) {
    lines.push("## 위반 지표");
    lines.push("");
    lines.push("| 리포트 | 지표 | 통계 | 종류 | 실측 | 한계 | 기준 | 변화 |");
    lines.push("|---|---|---|---|---:|---:|---:|---:|");
    for (const summary of summaries) {
      for (const violation of summary.violations) {
        lines.push(
          `| ${summary.platform}/${summary.suite} | \`${violation.metric}\` | ${violation.statistic} | ${violation.kind} | ${format(violation.actual)} | ${format(violation.limit)} | ${format(violation.baseline)} | ${formatPercent(violation.relativeChangePercent)} |`,
        );
      }
    }
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

  lines.push("## 이 이슈가 막고 있는 것");
  lines.push("");
  lines.push(
    `이 이슈가 \`${regressionLabel}\` 라벨을 달고 열려 있는 동안 \`Post-merge performance hold\` 체크가 실패한다. 해제하려면 회귀를 고치거나, 오탐으로 판단한 근거를 남기고 이슈를 닫거나 라벨을 제거한다.`,
  );

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
  const match = open.find((issue) => String(issue.body ?? "").includes(marker));
  return match ?? null;
}

export function buildRerunComment(input) {
  const lines = [
    "같은 커밋에서 병합 후 성능 런이 다시 실패했다.",
    "",
    `- 이벤트: \`${String(input.eventName ?? "push")}\``,
  ];
  if (input.runUrl) lines.push(`- 런: ${input.runUrl}`);
  const violations = violationCount(input.summaries ?? []);
  lines.push(`- 위반 지표 수: ${violations}`);
  return truncate(lines.join("\n"));
}

function collectJsonFiles(directory) {
  if (!directory) throw new Error("no report directory given");
  const stats = statSync(directory);
  if (!stats.isDirectory()) throw new Error(`${directory} is not a directory`);
  const found = [];
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && entry.name.endsWith(".json")) found.push(full);
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
      const { summaries, problems } = readEvaluatedReports(
        process.env.REPORT_DIRECTORY ?? "reports",
      );
      const issue = buildRegressionIssue({
        commit: process.env.REGRESSION_COMMIT,
        baselineCommit: process.env.REGRESSION_BASELINE,
        eventName: process.env.GITHUB_EVENT_NAME,
        runUrl: process.env.REGRESSION_RUN_URL,
        failedJobs,
        summaries,
        problems,
      });
      process.stdout.write(
        JSON.stringify({
          file: true,
          ...issue,
          comment: buildRerunComment({
            eventName: process.env.GITHUB_EVENT_NAME,
            runUrl: process.env.REGRESSION_RUN_URL,
            summaries,
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
