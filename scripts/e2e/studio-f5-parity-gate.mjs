#!/usr/bin/env node
// Release gate for #141: a real shared-project Studio F5 Runtime must pass a
// browser suite in an UNMODIFIED Chromium (`--asset-mirror off`).
//
// This gate never substitutes a fake WinBoat/HTTP fixture or Portable Runtime.
// It only runs where a real WinBoat VM with Studio Pro is configured (the
// self-hosted `winboat-studio` runner label). An assisted (mirrored) run is
// executed afterwards purely as comparison evidence; it can never satisfy the
// gate. Pure-navigation suites are rejected because widget-usability claims
// need representative state changes (#141).
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

const STATE_CHANGING_ACTIONS = new Set([
  "fill",
  "click",
  "selectOption",
  "press",
  "check",
  "uncheck",
]);
const VALUE_ASSERTION_ACTIONS = new Set(["expectText", "expectValue"]);

function required(name) {
  const value = process.env[name];
  assert(value, `set ${name}`);
  return value;
}

// The CLI prints one JSON envelope on stdout. A failed browser-test outcome is
// reported with exit code 1, so tolerate non-zero exits and parse the body.
async function cli(binary, args, timeoutMilliseconds) {
  try {
    const { stdout } = await execute(binary, args, {
      timeout: timeoutMilliseconds,
      maxBuffer: 16 * 1024 * 1024,
    });
    return { exitCode: 0, envelope: JSON.parse(stdout) };
  } catch (error) {
    if (error.stdout) {
      try {
        return {
          exitCode: error.code ?? 1,
          envelope: JSON.parse(error.stdout),
        };
      } catch {
        // Fall through to the opaque failure below.
      }
    }
    throw new Error("parity gate command failed or exceeded its deadline", {
      cause: error,
    });
  }
}

function collectSteps(suite) {
  const steps = [];
  for (const step of suite.beforeEach ?? []) steps.push(step);
  for (const test of suite.tests ?? []) {
    for (const step of test.steps ?? []) steps.push(step);
  }
  return steps;
}

export function validateSuiteShape(suite, suitePath) {
  assert.equal(
    suite.schemaVersion,
    "1.0.0",
    `${suitePath}: unsupported suite schema`,
  );
  assert(
    Array.isArray(suite.tests) && suite.tests.length > 0,
    "suite has no tests",
  );
  const steps = collectSteps(suite);
  assert(steps.length > 0, "suite has no steps");
  const actions = steps.map((step) => step.action);
  const stateChanging = actions.filter((action) =>
    STATE_CHANGING_ACTIONS.has(action),
  );
  const valueAssertions = actions.filter((action) =>
    VALUE_ASSERTION_ACTIONS.has(action),
  );
  assert(
    stateChanging.length > 0,
    "#141: parity suites need at least one state-changing action " +
      `(fill, click, selectOption, press, check, uncheck); got only [${actions.join(", ")}]`,
  );
  assert(
    valueAssertions.length > 0,
    "#141: parity suites need at least one value assertion (expectText, expectValue) " +
      "so widget usability is verified through observed state",
  );
}

export function correctionReport(summary) {
  const correction = (summary.corrections ?? []).find(
    (entry) => entry.kind === "host-lan-asset-mirror",
  );
  assert(
    correction,
    "browser summary must report the host-lan-asset-mirror correction",
  );
  return correction;
}

function runEvidence(summary) {
  return {
    outcome: summary.outcome,
    passed: summary.passed,
    failed: summary.failed,
    skipped: summary.skipped,
    browserParity: summary.browserParity,
    corrections: summary.corrections,
    sessionId: summary.sessionId,
  };
}

export async function runParityGate() {
  assert.equal(
    process.platform,
    "linux",
    "the Studio F5 parity gate requires Linux",
  );
  const binary = required("MENDIMARU_STUDIO_PARITY_BINARY");
  const suitePath = required("MENDIMARU_STUDIO_PARITY_SUITE");
  const evidencePath =
    process.env.MENDIMARU_STUDIO_PARITY_EVIDENCE ??
    "artifacts/e2e/studio-f5-parity.json";
  const runtimeTimeoutSeconds = Number(
    process.env.MENDIMARU_STUDIO_PARITY_RUNTIME_TIMEOUT_SECONDS ?? "90",
  );
  assert(
    Number.isInteger(runtimeTimeoutSeconds) && runtimeTimeoutSeconds > 0,
    "MENDIMARU_STUDIO_PARITY_RUNTIME_TIMEOUT_SECONDS must be a positive integer",
  );
  for (const value of [binary, suitePath]) assert(path.isAbsolute(value));

  const suiteBytes = await fs.readFile(suitePath);
  const suite = JSON.parse(suiteBytes.toString("utf8"));
  validateSuiteShape(suite, suitePath);

  const startedAt = new Date().toISOString();
  let runtimeSessionId = null;
  const runs = {};
  let failure;
  try {
    const start = await cli(
      binary,
      [
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        String(runtimeTimeoutSeconds),
      ],
      runtimeTimeoutSeconds * 1000 + 30_000,
    );
    if (start.exitCode !== 0) throw new Error("runtime start failed");
    runtimeSessionId =
      start.envelope.runtimeSessionId ??
      start.envelope.data?.runtime?.sessionId ??
      null;
    assert.match(runtimeSessionId ?? "", /^runtime_[a-f0-9]{32}$/);

    const wait = await cli(
      binary,
      [
        "runtime",
        "wait",
        "--session-id",
        runtimeSessionId,
        "--json",
        "--timeout-seconds",
        String(runtimeTimeoutSeconds),
      ],
      runtimeTimeoutSeconds * 1000 + 30_000,
    );
    if (wait.exitCode !== 0) {
      throw new Error(
        "Runtime session is not HTTP-ready; start Studio Pro Run Locally for the shared project in the guest",
      );
    }

    // Parity run: the gate's decision. The browser must stay unmodified.
    const unmodified = await cli(
      binary,
      [
        "browser",
        "test",
        "--runtime-session-id",
        runtimeSessionId,
        "--suite-path",
        suitePath,
        "--asset-mirror",
        "off",
        "--json",
        "--fail-on-console-error",
        "--fail-on-network-failure",
      ],
      10 * 60_000,
    );
    const unmodifiedSummary = unmodified.envelope.data;
    runs.unmodified = runEvidence(unmodifiedSummary);
    const unmodifiedCorrection = correctionReport(unmodifiedSummary);
    assert.equal(
      unmodifiedSummary.browserParity,
      "unmodified",
      "parity run must report an unmodified browser",
    );
    assert.equal(unmodifiedCorrection.applied, false);
    assert.equal(
      unmodifiedSummary.outcome,
      "passed",
      "the unmodified-browser Studio F5 path failed; an assisted pass is not ordinary-Chrome parity (#141)",
    );
    assert(unmodifiedSummary.passed > 0 && unmodifiedSummary.failed === 0);

    // Comparison evidence only: the assisted run can never satisfy this gate.
    const assisted = await cli(
      binary,
      [
        "browser",
        "test",
        "--runtime-session-id",
        runtimeSessionId,
        "--suite-path",
        suitePath,
        "--asset-mirror",
        "auto",
        "--json",
        "--fail-on-console-error",
        "--fail-on-network-failure",
      ],
      10 * 60_000,
    );
    runs.assisted = runEvidence(assisted.envelope.data);
    assert.equal(assisted.envelope.data.browserParity, "assisted");
    assert.equal(correctionReport(assisted.envelope.data).applied, true);
  } catch (error) {
    failure = error;
  } finally {
    if (runtimeSessionId) {
      await cli(
        binary,
        ["runtime", "stop", "--session-id", runtimeSessionId, "--json"],
        120_000,
      ).catch(() => {});
    }
  }

  const report = {
    issue: 141,
    outcome: failure ? "failed" : "passed",
    startedAt,
    finishedAt: new Date().toISOString(),
    suiteSha256: digest(suiteBytes),
    runtimeSessionId,
    runs,
  };
  await fs.mkdir(path.dirname(evidencePath), { recursive: true });
  await fs.writeFile(evidencePath, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report)}\n`);
  if (failure) throw failure;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  runParityGate().catch((error) => {
    process.stderr.write(
      `Studio F5 parity gate failed: ${error?.message ?? "unknown error"}\n`,
    );
    process.exitCode = 1;
  });
}
