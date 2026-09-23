import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

// The single place that answers "which reports must exist for this gate to
// have made a verdict at all".
//
// Issue #190: `release-webview-gate` is a matrix fan-in. One dead measurement
// leg used to collapse the whole gate into a skip, so `504c28f` reached main
// with no performance verdict on either platform. Adding `if: always()` alone
// would be worse, not better: the gate would run with no inputs and pass
// quietly, turning a visible red into an invisible green. The gate therefore
// has to know, independently of what it happens to find on disk, what it was
// supposed to be given.
//
// Issue #192: the post-merge safety net had the mirror-image defect. It read
// whatever `*.json` it found, so raw measurement dumps became "unusable"
// false alarms, while a gate verdict that never existed produced no message
// at all. Both halves are the same missing concept, so both read this module.

export const webviewPlatforms = ["linux", "windows"];
export const bundleKinds = ["msi", "nsis"];
export const gateVariants = ["baseline", "candidate"];
export const bundleScope = "installed-bundle";
export const gateScopes = [...webviewPlatforms, bundleScope];

export const gateManifestSchemaVersion = "1.0.0";

// The idle phase and the whole installed-bundle suite were moved past the
// merge point by the issue #176 split SLA. A pull request legitimately has no
// idle or bundle report, so expecting one there would turn E's relevance
// optimisation into a permanent red.
export function expectedPhases(eventName) {
  return String(eventName) === "pull_request"
    ? ["latency"]
    : ["latency", "idle"];
}

export function isBundleDeferred(eventName) {
  return String(eventName) === "pull_request";
}

function assertScope(scope) {
  if (!gateScopes.includes(String(scope))) {
    throw new Error(`unknown gate scope: ${JSON.stringify(scope)}`);
  }
  return String(scope);
}

// Inputs. What the gate has to read to be able to compare anything. Both
// variants are required: a missing baseline is exactly the `504c28f` failure,
// and comparing a candidate against nothing is not a verdict.
export function expectedMeasurementReports({ scope, eventName = "push" }) {
  const resolved = assertScope(scope);
  if (resolved === bundleScope) {
    if (isBundleDeferred(eventName)) return [];
    return bundleKinds
      .flatMap((kind) => gateVariants.map((v) => `${v}-${kind}.json`))
      .sort();
  }
  return expectedPhases(eventName)
    .flatMap((phase) =>
      gateVariants.map((v) => `${resolved}-${v}-${phase}.json`),
    )
    .sort();
}

// Outputs. The evaluated candidate reports the gate publishes and the
// post-merge safety net later quotes. Deliberately narrower than the input
// set, and deliberately an explicit list rather than a glob, so a raw
// measurement dump can never be mistaken for a verdict.
export function expectedGateReports({ scope, eventName = "push" }) {
  const resolved = assertScope(scope);
  if (resolved === bundleScope) {
    if (isBundleDeferred(eventName)) return [];
    return bundleKinds.map((kind) => `candidate-${kind}.json`).sort();
  }
  return expectedPhases(eventName)
    .map((phase) => `${resolved}-candidate-${phase}.json`)
    .sort();
}

export function gateManifestName(scope) {
  return `gate-manifest-${assertScope(scope)}.json`;
}

export function expectedGateManifests() {
  return gateScopes.map(gateManifestName);
}

// The closed whitelist the safety net reads with. Anything outside it is not
// a gate verdict and must not be parsed as one. `candidate-msi.raw.json` is
// excluded because it is not on this list, not because `.raw.json` is
// blacklisted -- a blacklist would break again on the next new file suffix.
export function knownGateReportNames() {
  const names = new Set();
  for (const scope of gateScopes) {
    for (const eventName of ["push", "pull_request"]) {
      for (const name of expectedGateReports({ scope, eventName })) {
        names.add(name);
      }
    }
  }
  return [...names].sort();
}

export function isGateReportName(basename) {
  return knownGateReportNames().includes(String(basename));
}

// A gate that did not run because its measured inputs cannot have changed is
// a correct pass, not a hole. `relevant` carries that distinction from the
// workflow into every consumer, so an intentional relevance skip is never
// reported as a missing verdict.
export function reconcileGateInputs({
  scope,
  eventName = "push",
  relevant = true,
  reason = "",
  found = [],
}) {
  const resolved = assertScope(scope);
  const skipped = relevant !== true;
  const expected = skipped
    ? []
    : expectedMeasurementReports({ scope: resolved, eventName });
  const expectedReports = skipped
    ? []
    : expectedGateReports({ scope: resolved, eventName });
  const present = [...new Set(found.map(String))].sort();
  const missing = expected.filter((name) => !present.includes(name));
  return {
    schemaVersion: gateManifestSchemaVersion,
    scope: resolved,
    eventName: String(eventName),
    relevant: !skipped,
    skipped,
    reason: String(reason ?? ""),
    expectedInputs: expected,
    expectedReports,
    used: expected.filter((name) => present.includes(name)),
    missing,
    ok: missing.length === 0,
  };
}

export function renderGateInputSummary(manifest) {
  const lines = [`### ${manifest.scope} gate inputs`, ""];
  if (manifest.skipped) {
    lines.push(
      `Skipped as not relevant${manifest.reason ? `: ${manifest.reason}` : ""}. No report is expected, so nothing is missing.`,
    );
    lines.push("");
    return `${lines.join("\n")}\n`;
  }
  lines.push(`- event: \`${manifest.eventName}\``);
  lines.push(
    `- verdict used: ${manifest.used.length > 0 ? manifest.used.map((n) => `\`${n}\``).join(", ") : "(none)"}`,
  );
  if (manifest.missing.length > 0) {
    lines.push(
      `- **missing: ${manifest.missing.map((n) => `\`${n}\``).join(", ")}**`,
    );
  }
  lines.push("");
  lines.push(
    manifest.ok
      ? "Every expected measurement report is present."
      : "An expected measurement report is absent, so this gate has no verdict and fails closed.",
  );
  lines.push("");
  return `${lines.join("\n")}\n`;
}

export function readDirectoryNames(directory) {
  try {
    if (!statSync(directory).isDirectory()) return [];
  } catch {
    return [];
  }
  const names = [];
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile()) names.push(entry.name);
    }
  };
  walk(directory);
  return [...new Set(names)].sort();
}

export function readGateManifests(directory) {
  const manifests = [];
  const problems = [];
  let entries;
  try {
    if (!statSync(directory).isDirectory()) return { manifests, problems };
    entries = readdirSync(directory, { withFileTypes: true });
  } catch {
    return { manifests, problems };
  }
  const wanted = new Set(expectedGateManifests());
  for (const entry of entries) {
    if (!entry.isFile() || !wanted.has(entry.name)) continue;
    try {
      const parsed = JSON.parse(
        readFileSync(path.join(directory, entry.name), "utf8"),
      );
      if (!parsed || typeof parsed !== "object" || !parsed.scope) {
        throw new Error("manifest has no scope");
      }
      manifests.push(parsed);
    } catch (error) {
      problems.push(
        `${entry.name}: unreadable gate manifest (${error instanceof Error ? error.message : String(error)})`,
      );
    }
  }
  manifests.sort((left, right) =>
    String(left.scope).localeCompare(String(right.scope)),
  );
  problems.sort();
  return { manifests, problems };
}

// What the post-merge safety net reports. A gate that never ran leaves no
// manifest, and that absence is itself the most important thing to say: it
// means no verdict exists, which is strictly worse than a verdict that failed.
export function missingGateVerdicts({ manifests = [], presentFiles = [] }) {
  const present = new Set(presentFiles.map(String));
  const byScope = new Map(
    manifests.map((manifest) => [String(manifest.scope), manifest]),
  );
  const missing = [];
  for (const scope of gateScopes) {
    const manifest = byScope.get(scope);
    if (!manifest) {
      missing.push({
        scope,
        kind: "no-verdict",
        detail: `게이트가 판정을 남기지 않았다 (\`${gateManifestName(scope)}\` 부재)`,
      });
      continue;
    }
    if (manifest.skipped === true) continue;
    for (const name of manifest.expectedReports ?? []) {
      if (!present.has(name)) {
        missing.push({
          scope,
          kind: "missing-report",
          detail: `평가된 게이트 리포트 \`${name}\` 이 없다`,
        });
      }
    }
    for (const name of manifest.missing ?? []) {
      missing.push({
        scope,
        kind: "missing-input",
        detail: `측정 리포트 \`${name}\` 이 없어 게이트가 비교하지 못했다`,
      });
    }
  }
  return missing;
}

// CLI used by the gate jobs. Writes the manifest first and only then fails,
// so the evidence of why the gate failed is uploaded even when it does.
const invokedDirectly =
  process.argv[1] && process.argv[1].endsWith("gate-reports.mjs");
if (invokedDirectly) {
  const directory = process.env.REPORT_DIRECTORY ?? "reports";
  const manifest = reconcileGateInputs({
    scope: process.env.GATE_SCOPE,
    eventName: process.env.GITHUB_EVENT_NAME ?? "push",
    relevant: String(process.env.GATE_RELEVANT ?? "true") === "true",
    reason: process.env.GATE_REASON ?? "",
    found: readDirectoryNames(directory),
  });
  const target = path.join(
    process.env.MANIFEST_DIRECTORY ?? directory,
    gateManifestName(manifest.scope),
  );
  writeFileSync(target, `${JSON.stringify(manifest, null, 2)}\n`);
  const summary = renderGateInputSummary(manifest);
  process.stdout.write(summary);
  if (process.env.GITHUB_STEP_SUMMARY) {
    const { appendFileSync } = await import("node:fs");
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  }
  if (!manifest.ok) {
    process.stderr.write(
      `expected measurement reports are missing: ${manifest.missing.join(", ")}\n`,
    );
    process.exitCode = 1;
  }
}
