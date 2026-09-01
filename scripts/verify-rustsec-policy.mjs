import fs from "node:fs";
import process from "node:process";

const DEFAULT_POLICY_PATH = "security/rustsec-exceptions.json";

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

export function verifyRustsecPolicy(report, policy, now = new Date()) {
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
    if (!/^\d{4}-\d{2}-\d{2}$/.test(exception.expiresAt ?? "")) {
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
  };
}

function parseArguments(argv) {
  const options = {
    policy: DEFAULT_POLICY_PATH,
    report: null,
    output: null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--policy") options.policy = argv[++index];
    else if (argument === "--report") options.report = argv[++index];
    else if (argument === "--output") options.output = argv[++index];
    else throw new Error(`unknown argument: ${argument}`);
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

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const reportSource = options.report
    ? fs.readFileSync(options.report, "utf8")
    : await readStdin();
  const report = parseJson(reportSource, "cargo audit JSON report");
  const policy = parseJson(
    fs.readFileSync(options.policy, "utf8"),
    "RustSec policy manifest",
  );
  const result = verifyRustsecPolicy(report, policy);
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
