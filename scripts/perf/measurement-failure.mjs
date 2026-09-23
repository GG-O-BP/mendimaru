// Issue #191. A release measurement job can fail for two very different
// reasons: the WebView harness lost its WebDriver session, or the measured
// contract broke. At check level both used to look the same - one red
// "Measure WebView (...)" box with a stack trace buried in the log - so a
// runner flake was indistinguishable from a real product problem.
//
// `release-webview.mjs` already writes a classified `*.failure.json`. This
// module lifts that classification into a GitHub annotation and the job
// summary. It is diagnostic only: it never changes an outcome, never retries,
// and never turns a failure green, so `preserve-original-failure` is intact.

import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { appendFile } from "node:fs/promises";

const FAILURE_SUFFIX = ".failure.json";

export function isFailureReportName(name) {
  return typeof name === "string" && name.endsWith(FAILURE_SUFFIX);
}

// Deterministic order and no duplicates: a latency job measures both variants,
// so more than one failure report can legitimately be present, and the summary
// must not depend on directory iteration order.
export async function collectFailureReports(directory) {
  let names;
  try {
    names = await readdir(directory);
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
  const unique = [...new Set(names.filter(isFailureReportName))].sort();
  const reports = [];
  for (const name of unique) {
    const file = path.join(directory, name);
    let report;
    try {
      report = JSON.parse(await readFile(file, "utf8"));
    } catch {
      report = undefined;
    }
    reports.push({ name, file, report });
  }
  return reports;
}

export function summariseFailure({ name, report }) {
  if (!report || typeof report !== "object") {
    return {
      name,
      classification: "unknown",
      reason: "unreadable-failure-report",
      stage: undefined,
      stageSample: undefined,
      command: undefined,
      headline: `${name}: the harness failure report could not be read`,
    };
  }
  const classification =
    typeof report.classification === "string"
      ? report.classification
      : "unknown";
  const reason = typeof report.reason === "string" ? report.reason : "unknown";
  const stage = typeof report.stage === "string" ? report.stage : undefined;
  const stageSample =
    typeof report.stageSample === "string" ? report.stageSample : undefined;
  const command = report.webdriverCommand ?? undefined;
  const where = stage
    ? `stage \`${stage}\`${stageSample ? ` sample ${stageSample}` : ""}`
    : "an unrecorded stage";
  return {
    name,
    classification,
    reason,
    stage,
    stageSample,
    command,
    headline: `${classificationLabel(classification)} (${reason}) at ${where}`,
  };
}

function classificationLabel(classification) {
  if (classification === "harness") return "측정 하네스 오류 — 성능 판정 아님";
  if (classification === "measurement") return "측정 계약 위반 — 확인 필요";
  return "분류 불가 — 확인 필요";
}

export function renderFailureSummary(entries) {
  if (entries.length === 0) {
    return "## 측정 실패 분류\n\n분류된 하네스 실패 리포트가 없습니다. 실패 원인은 잡 로그를 확인하세요.\n";
  }
  const lines = [
    "## 측정 실패 분류",
    "",
    "성능 게이트는 이 잡에서 실행되지 않았습니다. 아래 분류는 예산 판정이 아니라 측정 실행 자체의 실패 원인입니다.",
    "",
    "| 리포트 | 분류 | 사유 | 단계 | 샘플 | WebDriver 명령 |",
    "| --- | --- | --- | --- | --- | --- |",
  ];
  for (const entry of entries) {
    lines.push(
      `| \`${entry.name}\` | ${entry.classification} | ${entry.reason} | ${entry.stage ?? "—"} | ${entry.stageSample ?? "—"} | ${renderCommand(entry.command)} |`,
    );
  }
  lines.push("");
  return `${lines.join("\n")}\n`;
}

function renderCommand(command) {
  if (!command) return "—";
  const endpoint = command.endpoint ?? "unknown";
  const elapsed =
    command.elapsedMs === undefined ? "?" : `${command.elapsedMs} ms`;
  const scriptDeadline = command.scriptTimeoutMs ?? "inherited";
  return `\`${command.method ?? "?"} ${endpoint}\` ${elapsed}, script deadline ${scriptDeadline}`;
}

export function renderAnnotations(entries) {
  return entries.map(
    (entry) =>
      `::error title=${escapeAnnotation(`release performance ${entry.classification} failure`)}::${escapeAnnotation(entry.headline)}`,
  );
}

function escapeAnnotation(value) {
  return String(value)
    .replaceAll("%", "%25")
    .replaceAll("\r", "%0D")
    .replaceAll("\n", "%0A")
    .replaceAll("::", "%3A%3A");
}

export async function reportMeasurementFailure(directory, { write } = {}) {
  const entries = (await collectFailureReports(directory)).map((report) =>
    summariseFailure(report),
  );
  const annotations = renderAnnotations(entries);
  const summary = renderFailureSummary(entries);
  const emit = write ?? ((text) => process.stdout.write(text));
  for (const annotation of annotations) emit(`${annotation}\n`);
  emit(summary);
  return { entries, annotations, summary };
}

if (process.argv[1] && import.meta.url === `file://${process.argv[1]}`) {
  const directory = process.argv[2] ?? "artifacts/e2e/release-performance";
  const { summary } = await reportMeasurementFailure(directory);
  if (process.env.GITHUB_STEP_SUMMARY) {
    await appendFile(process.env.GITHUB_STEP_SUMMARY, summary).catch(
      () => undefined,
    );
  }
  // Diagnostic only. The measurement step already failed the job; exiting
  // non-zero here would only mask which step actually broke.
  process.exit(0);
}
