import fs from "node:fs";
import process from "node:process";

const DEFAULT_POLICY_PATH = "security/rustsec-exceptions.json";
const DAY_MS = 24 * 60 * 60 * 1000;

// An exception expiring within this window is reported as a warning so its
// owner can re-review it before the gate turns red on an unchanged lockfile.
export const EXPIRY_WARNING_DAYS = 30;

function isoDay(date) {
  return date.toISOString().slice(0, 10);
}

export function isValidIsoDate(value) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    return false;
  }
  const parsed = new Date(`${value}T00:00:00.000Z`);
  return !Number.isNaN(parsed.getTime()) && isoDay(parsed) === value;
}

function exceptionKey(exception) {
  return `${exception?.kind}:${exception?.advisoryId}`;
}

/**
 * Date-only check of every policy exception. It needs no cargo-audit report,
 * so it can run on every pull request even when the dependency audit itself
 * is skipped by path relevance: an expiry is caused by the calendar, not by a
 * changed path (#228).
 */
export function verifyExceptionExpiry(
  policy,
  now = new Date(),
  warningDays = EXPIRY_WARNING_DAYS,
) {
  const errors = [];
  const expiringSoon = [];
  if (!Array.isArray(policy?.exceptions)) {
    errors.push("RustSec policy manifest has no exceptions list");
    return { ok: false, exceptionCount: 0, errors, expiringSoon };
  }
  const today = isoDay(now);
  const todayMs = Date.parse(`${today}T00:00:00.000Z`);
  for (const exception of policy.exceptions) {
    const key = exceptionKey(exception);
    const expiresAt = exception?.expiresAt;
    if (!isValidIsoDate(expiresAt)) {
      errors.push(`${key} has an invalid expiresAt date`);
      continue;
    }
    if (today > expiresAt) {
      errors.push(`policy exception expired on ${expiresAt}: ${key}`);
      continue;
    }
    const daysRemaining = Math.round(
      (Date.parse(`${expiresAt}T00:00:00.000Z`) - todayMs) / DAY_MS,
    );
    if (daysRemaining <= warningDays) {
      expiringSoon.push({ key, expiresAt, daysRemaining });
    }
  }
  return {
    ok: errors.length === 0,
    exceptionCount: policy.exceptions.length,
    errors,
    expiringSoon,
  };
}

export function collectRustsecWarnings(report) {
  const warnings = [];
  for (const warning of report.warnings?.unmaintained ?? []) {
    warnings.push({
      key: `unmaintained:${warning.advisory.id}`,
      kind: "unmaintained",
      advisoryId: warning.advisory.id,
      packageName: warning.package.name,
      packageVersion: warning.package.version,
    });
  }
  for (const warning of report.warnings?.unsound ?? []) {
    warnings.push({
      key: `unsound:${warning.advisory.id}`,
      kind: "unsound",
      advisoryId: warning.advisory.id,
      packageName: warning.package.name,
      packageVersion: warning.package.version,
    });
  }
  for (const warning of report.warnings?.yanked ?? []) {
    warnings.push({
      key: `yanked:${warning.package.name}@${warning.package.version}`,
      kind: "yanked",
      advisoryId: null,
      packageName: warning.package.name,
      packageVersion: warning.package.version,
    });
  }
  return warnings;
}

export function verifyRustsecPolicy(
  report,
  policy,
  now = new Date(),
  warningDays = EXPIRY_WARNING_DAYS,
) {
  const errors = [];
  const vulnerabilities = report.vulnerabilities?.list ?? [];
  for (const vulnerability of vulnerabilities) {
    errors.push(
      `vulnerability denied: ${vulnerability.advisory.id} in ${vulnerability.package.name}@${vulnerability.package.version}`,
    );
  }

  const warnings = collectRustsecWarnings(report);
  const warningKeys = new Set(warnings.map((warning) => warning.key));
  const exceptions = new Map();
  for (const exception of policy.exceptions ?? []) {
    const key = `${exception.kind}:${exception.advisoryId}`;
    if (exceptions.has(key)) errors.push(`duplicate policy exception: ${key}`);
    exceptions.set(key, exception);
  }

  for (const warning of warnings) {
    if (!exceptions.has(warning.key)) {
      errors.push(
        `unknown RustSec warning denied: ${warning.key} (${warning.packageName}@${warning.packageVersion})`,
      );
    }
  }

  const today = now.toISOString().slice(0, 10);
  for (const [key, exception] of exceptions) {
    if (!warningKeys.has(key)) {
      errors.push(`stale policy exception no longer present: ${key}`);
      continue;
    }
    for (const field of [
      "advisoryId",
      "kind",
      "owner",
      "reason",
      "upstreamUrl",
      "remediationPlan",
      "expiresAt",
    ]) {
      if (typeof exception[field] !== "string" || !exception[field].trim()) {
        errors.push(`${key} is missing ${field}`);
      }
    }
    if (
      !Array.isArray(exception.introducedBy) ||
      exception.introducedBy.length === 0
    ) {
      errors.push(`${key} must record at least one introducing dependency`);
    }
    if (!isValidIsoDate(exception.expiresAt)) {
      errors.push(`${key} has an invalid expiresAt date`);
    } else if (today > exception.expiresAt) {
      errors.push(`policy exception expired on ${exception.expiresAt}: ${key}`);
    }
    if (
      !URL.canParse(exception.upstreamUrl) ||
      !exception.upstreamUrl.startsWith("https://")
    ) {
      errors.push(`${key} upstreamUrl must be an HTTPS URL`);
    }

    const warning = warnings.find((candidate) => candidate.key === key);
    if (
      warning &&
      (warning.packageName !== exception.package?.name ||
        warning.packageVersion !== exception.package?.version)
    ) {
      errors.push(
        `${key} is pinned to ${exception.package?.name}@${exception.package?.version}, but the lockfile has ${warning.packageName}@${warning.packageVersion}`,
      );
    }

    if (exception.kind === "unsound") {
      if (exception.exceptionClass !== "bounded-reachability") {
        errors.push(`${key} uses an unsupported unsound exception class`);
      }
      if (
        exception.reachability?.status !== "not-reachable" ||
        typeof exception.reachability?.evidence !== "string" ||
        !exception.reachability.evidence.trim() ||
        typeof exception.reachability?.checkedCommit !== "string" ||
        !exception.reachability.checkedCommit.trim()
      ) {
        errors.push(`${key} lacks complete non-reachability evidence`);
      }
    } else if (exception.kind !== "unmaintained") {
      errors.push(`${key} has an unsupported exception kind`);
    }
  }

  for (const warning of warnings.filter((item) => item.kind === "yanked")) {
    errors.push(
      `yanked dependency denied: ${warning.packageName}@${warning.packageVersion}`,
    );
  }

  const expiringSoon = verifyExceptionExpiry(
    { exceptions: [...exceptions.values()] },
    now,
    warningDays,
  ).expiringSoon.filter((entry) => warningKeys.has(entry.key));

  return {
    ok: errors.length === 0,
    vulnerabilityCount: vulnerabilities.length,
    warningCount: warnings.length,
    activeExceptionCount: warnings.filter((warning) =>
      exceptions.has(warning.key),
    ).length,
    staleExceptionCount: [...exceptions.keys()].filter(
      (key) => !warningKeys.has(key),
    ).length,
    errors,
    expiringSoon,
  };
}

/**
 * Removes only exceptions whose advisory no longer appears in the audit
 * report (#237). A compatible dependency update that resolves an advisory
 * must not be blocked by its now-unnecessary exception. This never adds,
 * extends, or edits an exception: every remaining exception keeps its exact
 * metadata and is still verified by verifyRustsecPolicy afterwards.
 */
export function pruneStaleExceptions(report, policy) {
  if (!Array.isArray(policy?.exceptions)) {
    throw new Error("RustSec policy manifest has no exceptions list");
  }
  const present = new Set(
    collectRustsecWarnings(report).map((warning) => warning.key),
  );
  const kept = [];
  const removed = [];
  for (const exception of policy.exceptions) {
    const key = exceptionKey(exception);
    if (present.has(key)) kept.push(exception);
    else removed.push(key);
  }
  return { policy: { ...policy, exceptions: kept }, removed };
}

function parseArguments(argv) {
  const options = {
    policy: DEFAULT_POLICY_PATH,
    report: null,
    output: null,
    expiryOnly: false,
    pruneStale: false,
    today: null,
    warningDays: EXPIRY_WARNING_DAYS,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--policy") options.policy = argv[++index];
    else if (argument === "--report") options.report = argv[++index];
    else if (argument === "--output") options.output = argv[++index];
    else if (argument === "--expiry-only") options.expiryOnly = true;
    else if (argument === "--prune-stale") options.pruneStale = true;
    else if (argument === "--today") options.today = argv[++index];
    else if (argument === "--warning-days") {
      const value = argv[++index];
      if (!/^\d{1,4}$/.test(value ?? "")) {
        throw new Error("--warning-days requires a non-negative integer");
      }
      options.warningDays = Number(value);
    } else throw new Error(`unknown argument: ${argument}`);
  }
  if (options.today !== null && !isValidIsoDate(options.today)) {
    throw new Error("--today requires a YYYY-MM-DD date");
  }
  if (options.expiryOnly && options.report) {
    throw new Error("--expiry-only does not read a cargo audit report");
  }
  if (options.pruneStale && (options.expiryOnly || options.output)) {
    throw new Error(
      "--prune-stale rewrites only the policy and cannot be combined with --expiry-only or --output",
    );
  }
  return options;
}

async function readStdin() {
  let input = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) input += chunk;
  return input;
}

function parseJson(source, label) {
  try {
    return JSON.parse(source);
  } catch (error) {
    throw new Error(`${label} is not valid JSON: ${error.message}`, {
      cause: error,
    });
  }
}

export function formatExpiryWarning(entry, githubActions) {
  const message = `RustSec policy exception ${entry.key} expires on ${entry.expiresAt} (${entry.daysRemaining} day(s) left); re-review it before then`;
  return githubActions
    ? `::warning title=RustSec exception expiring::${message}`
    : `warning: ${message}`;
}

function writeExpiryWarnings(expiringSoon) {
  // GITHUB_* variables cannot be overridden per step, so the Markdown step
  // summary opts out of workflow-command output with its own variable.
  const githubActions =
    process.env.GITHUB_ACTIONS === "true" &&
    process.env.RUSTSEC_WARNING_FORMAT !== "plain";
  for (const entry of expiringSoon) {
    process.stdout.write(`${formatExpiryWarning(entry, githubActions)}\n`);
  }
}

function readPolicy(path) {
  return parseJson(fs.readFileSync(path, "utf8"), "RustSec policy manifest");
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const now = options.today
    ? new Date(`${options.today}T00:00:00.000Z`)
    : new Date();
  if (options.expiryOnly) {
    const result = verifyExceptionExpiry(
      readPolicy(options.policy),
      now,
      options.warningDays,
    );
    process.stdout.write(
      `RustSec exception expiry: ${result.ok ? "PASS" : "FAIL"} — exceptions=${result.exceptionCount}, expiringSoon=${result.expiringSoon.length}\n`,
    );
    writeExpiryWarnings(result.expiringSoon);
    if (!result.ok) {
      for (const error of result.errors) process.stderr.write(`${error}\n`);
      process.exitCode = 1;
    }
    return;
  }
  const reportSource = options.report
    ? fs.readFileSync(options.report, "utf8")
    : await readStdin();
  const report = parseJson(reportSource, "cargo audit JSON report");
  let policy = readPolicy(options.policy);
  if (options.pruneStale) {
    const pruned = pruneStaleExceptions(report, policy);
    if (pruned.removed.length > 0) {
      // Write through a temporary file so a crash never truncates the policy.
      const temporary = `${options.policy}.tmp`;
      fs.writeFileSync(
        temporary,
        `${JSON.stringify(pruned.policy, null, 2)}\n`,
      );
      fs.renameSync(temporary, options.policy);
    }
    process.stdout.write(
      `RustSec stale exceptions removed: ${pruned.removed.length}${pruned.removed.length ? ` (${pruned.removed.join(", ")})` : ""}\n`,
    );
    policy = pruned.policy;
  }
  const result = verifyRustsecPolicy(report, policy, now, options.warningDays);
  const artifact = {
    schemaVersion: 1,
    checkedAt: new Date().toISOString(),
    policyPath: options.policy,
    ...result,
  };
  if (options.output) {
    const output = new URL(options.output, `file://${process.cwd()}/`);
    fs.mkdirSync(new URL(".", output), { recursive: true });
    fs.writeFileSync(output, `${JSON.stringify(artifact, null, 2)}\n`);
  }
  process.stdout.write(
    `RustSec policy: ${result.ok ? "PASS" : "FAIL"} — vulnerabilities=${result.vulnerabilityCount}, warnings=${result.warningCount}, exceptions=${result.activeExceptionCount}, stale=${result.staleExceptionCount}\n`,
  );
  writeExpiryWarnings(result.expiringSoon);
  if (!result.ok) {
    for (const error of result.errors) process.stderr.write(`${error}\n`);
    process.exitCode = 1;
  }
}

if (
  process.argv[1] &&
  import.meta.url === new URL(`file://${process.argv[1]}`).href
) {
  await main();
}
