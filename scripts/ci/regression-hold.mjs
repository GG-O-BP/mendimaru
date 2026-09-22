import process from "node:process";

import { regressionLabel, revertCandidateLabel } from "./regression-issue.mjs";

// Counterpart to the auto-filed issue. Deferring idle and installed-bundle
// detection past the merge point is only safe if a detected regression stops
// more commits from stacking on top of it, so this check reads the open
// regression issues and reports a hold.
//
// Reading a list rather than calling the API keeps the decision testable and
// keeps the GitHub query in one visible place in the workflow.
export function evaluateHold(issues) {
  if (!Array.isArray(issues)) {
    // Fail closed. A malformed query result must not read as "nothing is
    // broken"; the check goes red and a human looks at it.
    throw new Error("expected an array of issues");
  }
  const blocking = issues
    .filter((issue) => issue && typeof issue === "object")
    // `gh issue list` already excludes pull requests, but a hand-supplied or
    // future query might not, and a PR must never hold merges.
    .filter((issue) => !issue.pull_request && !issue.isPullRequest)
    .filter((issue) => String(issue.state ?? "OPEN").toLowerCase() === "open")
    .filter((issue) => labelNames(issue).includes(regressionLabel))
    .map((issue) => ({
      number: Number(issue.number),
      title: String(issue.title ?? ""),
      url: String(issue.url ?? issue.html_url ?? ""),
      revertCandidate: labelNames(issue).includes(revertCandidateLabel),
    }))
    .filter((issue) => Number.isInteger(issue.number))
    .sort((left, right) => left.number - right.number);
  return { held: blocking.length > 0, issues: blocking };
}

export function renderHold(decision) {
  if (!decision.held) {
    return `Post-merge performance hold: clear (no open \`${regressionLabel}\` issue).`;
  }
  const lines = [
    `Post-merge performance hold: ACTIVE — ${decision.issues.length} open \`${regressionLabel}\` issue(s).`,
    "",
  ];
  for (const issue of decision.issues) {
    const marker = issue.revertCandidate ? " [revert candidate]" : "";
    lines.push(`  #${issue.number}${marker} ${issue.title}`);
    if (issue.url) lines.push(`    ${issue.url}`);
  }
  lines.push("");
  lines.push(
    `main is carrying an unresolved post-merge performance failure. Fix it, or close the issue / remove the \`${regressionLabel}\` label with a recorded justification, then re-run this check.`,
  );
  return lines.join("\n");
}

function labelNames(issue) {
  const labels = issue.labels ?? [];
  if (!Array.isArray(labels)) return [];
  return labels
    .map((label) => (typeof label === "string" ? label : (label?.name ?? "")))
    .map((name) => String(name));
}

const invokedDirectly =
  process.argv[1] && process.argv[1].endsWith("regression-hold.mjs");
if (invokedDirectly) {
  const raw = await readStdin();
  const decision = evaluateHold(JSON.parse(raw.trim() === "" ? "[]" : raw));
  process.stdout.write(`${renderHold(decision)}\n`);
  if (decision.held) process.exitCode = 1;
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
