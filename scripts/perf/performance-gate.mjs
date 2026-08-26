import { appendFile, readFile, writeFile } from "node:fs/promises";
import process from "node:process";

import {
  evaluatePerformance,
  renderPerformanceMarkdown,
  validatePerformancePolicy,
  validatePerformanceReport,
} from "./performance-core.mjs";

const [candidatePath, baselinePath, policyPath = "performance/budgets.json"] =
  process.argv.slice(2);
if (!candidatePath || !baselinePath) {
  throw new Error(
    "usage: node scripts/perf/performance-gate.mjs <candidate.json> <baseline.json> [budgets.json]",
  );
}

const [candidate, baseline, policy] = await Promise.all([
  readJson(candidatePath),
  readJson(baselinePath),
  readJson(policyPath),
]);
validatePerformanceReport(candidate);
validatePerformanceReport(baseline);
validatePerformancePolicy(policy);
const gate = evaluatePerformance(candidate, baseline, policy);
await writeFile(candidatePath, `${JSON.stringify(candidate, null, 2)}\n`);
const summary = renderPerformanceMarkdown(candidate);
process.stdout.write(summary);
if (process.env.GITHUB_STEP_SUMMARY) {
  await appendFile(process.env.GITHUB_STEP_SUMMARY, summary);
}
if (gate.status !== "passed") process.exitCode = 1;

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}
