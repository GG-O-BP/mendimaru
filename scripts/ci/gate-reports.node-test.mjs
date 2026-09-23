import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";

import {
  bundleScope,
  expectedGateManifests,
  expectedGateReports,
  expectedMeasurementReports,
  gateManifestName,
  gateScopes,
  isGateReportName,
  knownGateReportNames,
  missingGateVerdicts,
  readGateManifests,
  reconcileGateInputs,
  renderGateInputSummary,
  webviewPlatforms,
} from "./gate-reports.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

function scratch() {
  return mkdtempSync(path.join(tmpdir(), "mendimaru-gate-reports-"));
}

test("a push expects both variants of both phases on both platforms", () => {
  assert.deepEqual(expectedMeasurementReports({ scope: "linux" }), [
    "linux-baseline-idle.json",
    "linux-baseline-latency.json",
    "linux-candidate-idle.json",
    "linux-candidate-latency.json",
  ]);
});

test("a pull request expects only latency, because #176 deferred idle", () => {
  // Expecting an idle report on a pull request would turn the split SLA into a
  // permanent red and undo the relevance optimisation it protects.
  assert.deepEqual(
    expectedMeasurementReports({ scope: "windows", eventName: "pull_request" }),
    ["windows-baseline-latency.json", "windows-candidate-latency.json"],
  );
  assert.deepEqual(
    expectedGateReports({ scope: "windows", eventName: "pull_request" }),
    ["windows-candidate-latency.json"],
  );
});

test("the installed-bundle suite expects nothing on a pull request", () => {
  assert.deepEqual(
    expectedMeasurementReports({
      scope: bundleScope,
      eventName: "pull_request",
    }),
    [],
  );
  assert.deepEqual(expectedMeasurementReports({ scope: bundleScope }), [
    "baseline-msi.json",
    "baseline-nsis.json",
    "candidate-msi.json",
    "candidate-nsis.json",
  ]);
});

test("an unknown scope is rejected rather than silently expecting nothing", () => {
  // Returning an empty expectation for a typo would make the gate pass on no
  // evidence, which is the exact failure mode #190 is about.
  assert.throws(
    () => expectedMeasurementReports({ scope: "darwin" }),
    /unknown gate scope/,
  );
});

test("the whitelist excludes raw dumps without blacklisting a suffix", () => {
  assert.ok(isGateReportName("candidate-msi.json"));
  assert.ok(isGateReportName("linux-candidate-idle.json"));
  assert.ok(!isGateReportName("candidate-msi.raw.json"));
  assert.ok(!isGateReportName("candidate-nsis.raw.json"));
  // Baseline reports are gate inputs, never published verdicts.
  assert.ok(!isGateReportName("linux-baseline-latency.json"));
  assert.ok(!isGateReportName("skip-reason.txt"));
  assert.ok(!isGateReportName("gate-manifest-linux.json"));
  assert.equal(new Set(knownGateReportNames()).size, 6);
});

// The `504c28f` scenario, from run 35743578471: `Measure WebView (linux /
// baseline / idle)` died, every other leg passed, and the gate produced no
// verdict for either platform.
test("504c28f: the linux gate fails closed when one baseline leg is absent", () => {
  const manifest = reconcileGateInputs({
    scope: "linux",
    eventName: "push",
    relevant: true,
    found: [
      "linux-baseline-latency.json",
      "linux-candidate-latency.json",
      "linux-candidate-idle.json",
    ],
  });
  assert.equal(manifest.ok, false);
  assert.deepEqual(manifest.missing, ["linux-baseline-idle.json"]);
  assert.ok(manifest.used.includes("linux-candidate-idle.json"));
});

test("504c28f: the windows gate still returns its own verdict", () => {
  // The whole point of the fix. One dead Linux leg must not erase a Windows
  // verdict that was measured successfully.
  const manifest = reconcileGateInputs({
    scope: "windows",
    eventName: "push",
    relevant: true,
    found: [
      "windows-baseline-idle.json",
      "windows-baseline-latency.json",
      "windows-candidate-idle.json",
      "windows-candidate-latency.json",
    ],
  });
  assert.equal(manifest.ok, true);
  assert.deepEqual(manifest.missing, []);
  assert.equal(manifest.used.length, 4);
});

test("an empty report directory fails closed instead of passing quietly", () => {
  // `if: always()` alone would have produced exactly this state: the gate runs,
  // finds nothing to check, and reports success. That is worse than the skip.
  const manifest = reconcileGateInputs({
    scope: "linux",
    eventName: "push",
    relevant: true,
    found: [],
  });
  assert.equal(manifest.ok, false);
  assert.equal(manifest.missing.length, 4);
});

test("an intentional relevance skip expects nothing and passes", () => {
  const manifest = reconcileGateInputs({
    scope: "linux",
    eventName: "pull_request",
    relevant: false,
    reason: "no measured-artifact paths changed",
    found: [],
  });
  assert.equal(manifest.ok, true);
  assert.equal(manifest.skipped, true);
  assert.deepEqual(manifest.expectedInputs, []);
  assert.ok(renderGateInputSummary(manifest).includes("nothing is missing"));
});

test("the summary names the reports the verdict rests on", () => {
  const manifest = reconcileGateInputs({
    scope: "windows",
    eventName: "pull_request",
    relevant: true,
    found: ["windows-baseline-latency.json", "windows-candidate-latency.json"],
  });
  const summary = renderGateInputSummary(manifest);
  assert.ok(summary.includes("windows-candidate-latency.json"));
  assert.ok(summary.includes("Every expected measurement report is present."));
});

test("a scope with no manifest is reported as having no verdict at all", () => {
  const missing = missingGateVerdicts({ manifests: [], presentFiles: [] });
  assert.deepEqual(
    missing.map((entry) => entry.scope),
    gateScopes,
  );
  assert.ok(missing.every((entry) => entry.kind === "no-verdict"));
});

test("a published manifest whose report never arrived is reported too", () => {
  const manifest = reconcileGateInputs({
    scope: "linux",
    eventName: "pull_request",
    relevant: true,
    found: ["linux-baseline-latency.json", "linux-candidate-latency.json"],
  });
  const missing = missingGateVerdicts({
    manifests: [manifest],
    presentFiles: [],
  });
  assert.ok(
    missing.some(
      (entry) =>
        entry.scope === "linux" &&
        entry.kind === "missing-report" &&
        entry.detail.includes("linux-candidate-latency.json"),
    ),
  );
});

test("manifests round-trip through disk and unreadable ones are recorded", () => {
  const directory = scratch();
  const manifest = reconcileGateInputs({
    scope: "linux",
    eventName: "push",
    relevant: true,
    found: expectedMeasurementReports({ scope: "linux" }),
  });
  writeFileSync(
    path.join(directory, gateManifestName("linux")),
    JSON.stringify(manifest),
  );
  writeFileSync(path.join(directory, gateManifestName("windows")), "{ broken");
  const result = readGateManifests(directory);
  assert.equal(result.manifests.length, 1);
  assert.equal(result.manifests[0].scope, "linux");
  assert.equal(result.problems.length, 1);
  assert.ok(result.problems[0].includes("gate-manifest-windows.json"));
});

test("every expected manifest name is one the workflow actually uploads", () => {
  const workflow = readFileSync(
    path.join(repository, ".github", "workflows", "release-performance.yml"),
    "utf8",
  );
  for (const name of expectedGateManifests()) {
    // The two WebView manifests are uploaded through the matrix expression, so
    // the literal platform name never appears in the workflow text.
    const uploaded = webviewPlatforms.some((platform) =>
      name.endsWith(`-${platform}.json`),
    )
      ? "gate-manifest-${{ matrix.platform }}.json"
      : name;
    assert.ok(
      workflow.includes(name) || workflow.includes(uploaded),
      `${name} is expected but never uploaded`,
    );
  }
});

test("both fan-in gates carry the always() guard that #190 was missing", () => {
  // Guards against a future edit quietly dropping the guard and restoring the
  // skip cascade without any test noticing.
  const workflow = readFileSync(
    path.join(repository, ".github", "workflows", "release-performance.yml"),
    "utf8",
  );
  for (const job of ["release-webview-gate:", "installed-bundle-gate:"]) {
    const start = workflow.indexOf(`\n  ${job}`);
    assert.ok(start > 0, `${job} not found`);
    const block = workflow.slice(start, start + 900);
    assert.ok(
      block.includes("if: always() && !cancelled()"),
      `${job} must expand its matrix even when an upstream leg fails`,
    );
  }
});

test("the bundle gate artifact no longer globs the raw dumps", () => {
  const workflow = readFileSync(
    path.join(repository, ".github", "workflows", "release-performance.yml"),
    "utf8",
  );
  assert.ok(
    !workflow.includes("path: reports/candidate-*.json"),
    "reports/candidate-*.json also matches candidate-msi.raw.json",
  );
});

test("every webview platform has a manifest name and they are all distinct", () => {
  const names = webviewPlatforms.map(gateManifestName);
  assert.equal(new Set(names).size, names.length);
  assert.ok(expectedGateManifests().includes(gateManifestName(bundleScope)));
});
