import { execFileSync } from "node:child_process";
import { tauriResourceInputs } from "../perf/release-relevance.mjs";

// This is deliberately a closed list, independent of the PR skip classifier.
// Unknown paths, unreadable diffs and unknown suites retain the revert signal.
// These files cannot change the installed binary or its measurement contract.
const installedBundleOnlyExclusions = new Set([
  "performance/budgets.latency.json",
]);

function excludedPath(file, suite) {
  if (
    typeof file !== "string" ||
    file.split("/").some((part) => part === ".." || part === "." || !part)
  )
    return false;
  // Config changes are never excluded. Protect files already packaged by the
  // current config as well, even if they happen to live in a documentation tree.
  if (
    tauriResourceInputs().some(
      (resource) => file === resource || file.startsWith(`${resource}/`),
    )
  )
    return false;
  return (
    /^docs\/[\w/-]+\.md$/.test(file) ||
    /^scripts\/(?:ci|perf)\/[^/]+\.node-test\.mjs$/.test(file) ||
    /^scripts\/perf\/fixtures\/[^/]+\.json$/.test(file) ||
    (suite === "installed-bundle" && installedBundleOnlyExclusions.has(file))
  );
}

export function unchangedViolationInputs(input) {
  const { changedPaths, summaries = [], problems = [], missing = [] } = input;
  if (
    !Array.isArray(changedPaths) ||
    changedPaths.length === 0 ||
    problems.length > 0 ||
    missing.length > 0
  )
    return false;
  const failed = summaries.filter((summary) => summary.violations.length > 0);
  return (
    failed.length > 0 &&
    failed.every(
      (summary) =>
        summary.suite === "installed-bundle" &&
        summary.commit === input.commit &&
        summary.baselineCommit === input.baselineCommit &&
        changedPaths.every((file) => excludedPath(file, summary.suite)),
    )
  );
}

export function readAttributionPaths(baseline, commit, cwd) {
  if (![baseline, commit].every((sha) => /^[0-9a-f]{40}$/.test(sha ?? ""))) {
    throw new Error("attribution requires full baseline and candidate commits");
  }
  return execFileSync(
    "git",
    ["diff", "--name-only", "--no-renames", "-z", baseline, commit, "--"],
    { cwd, encoding: "utf8" },
  )
    .split("\0")
    .filter(Boolean);
}
