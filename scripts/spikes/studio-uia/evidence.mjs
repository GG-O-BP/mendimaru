import assert from "node:assert/strict";

export const steps = [
  "open-page",
  "select-widget",
  "change-property",
  "design-mode",
  "structure-mode",
  "synchronize-f4",
  "run-locally",
];
const statuses = new Set(["pass", "fail", "blocked", "not-run"]);
const transports = new Set(["linux-winboat", "windows-native"]);
const token = /^[a-z][a-z0-9-]{0,79}$/;

function artifact(value) {
  assert(
    typeof value === "string" &&
      /^[a-zA-Z0-9][a-zA-Z0-9_./-]{0,239}$/.test(value) &&
      !value.split("/").some((part) => part === ".." || part === "."),
    "artifact must be a relative path without traversal",
  );
}

// This validates the campaign ledger, not the truth of a UI assertion. A human
// must review its private artifacts before publishing a feasibility decision.
export function summarize(campaign) {
  assert.equal(campaign.schemaVersion, 1);
  assert(transports.has(campaign.transport), "unknown transport");
  assert.equal(campaign.nativeWindows, "deferred");
  assert.equal(
    campaign.transport,
    "linux-winboat",
    "native is deferred in #16",
  );
  assert.equal(campaign.targets.length, 2, "one exact 10.x and 11.x target");
  const versions = new Set();
  const targets = campaign.targets.map((target) => {
    assert.match(target.version, /^(10|11)\.\d+\.\d+(?:\.\d+)?$/);
    assert(!versions.has(target.version.split(".")[0]), "duplicate major");
    versions.add(target.version.split(".")[0]);
    assert.match(target.projectSha256, /^[a-f0-9]{64}$/);
    assert.equal(target.trials.length, 20, "retain all 20 planned trials");
    const ids = new Set();
    const failures = {};
    const counts = { pass: 0, fail: 0, blocked: 0, "not-run": 0 };
    const phaseCounts = {
      cold: { planned: 0, executed: 0, passed: 0 },
      warm: { planned: 0, executed: 0, passed: 0 },
    };
    for (const trial of target.trials) {
      assert(Number.isInteger(trial.id) && trial.id >= 1 && trial.id <= 20);
      assert(!ids.has(trial.id), "duplicate trial");
      ids.add(trial.id);
      assert(["cold", "warm"].includes(trial.phase), "unknown phase");
      phaseCounts[trial.phase].planned++;
      assert.equal(trial.steps.length, steps.length);
      assert.deepEqual(
        trial.steps.map((step) => step.id).sort(),
        [...steps].sort(),
      );
      let executed = false;
      for (const step of trial.steps) {
        assert(statuses.has(step.status), "unknown step status");
        assert(
          Number.isFinite(step.elapsedMs) && step.elapsedMs >= 0,
          "invalid duration",
        );
        assert(Array.isArray(step.artifacts));
        step.artifacts.forEach(artifact);
        if (step.status === "pass" || step.status === "fail") {
          executed = true;
          assert(step.artifacts.length > 0, "executed step requires evidence");
        }
        if (step.status === "pass") {
          assert.equal(
            step.effectVerified,
            true,
            "exit 0 is not an effect check",
          );
        } else {
          assert.match(step.reason, token, "non-pass requires a stable reason");
          failures[step.reason] = (failures[step.reason] ?? 0) + 1;
        }
      }
      const state = trial.steps.every((step) => step.status === "pass")
        ? "pass"
        : trial.steps.some((step) => step.status === "fail")
          ? "fail"
          : trial.steps.some((step) => step.status === "blocked")
            ? "blocked"
            : "not-run";
      counts[state]++;
      if (executed) phaseCounts[trial.phase].executed++;
      if (state === "pass") phaseCounts[trial.phase].passed++;
    }
    assert(phaseCounts.cold.planned > 0 && phaseCounts.warm.planned > 0);
    const executed = phaseCounts.cold.executed + phaseCounts.warm.executed;
    return {
      version: target.version,
      planned: 20,
      executed,
      counts,
      phaseCounts,
      // Null is intentional: an inaccessible VM is not a measured UI failure.
      successRate: executed === 0 ? null : counts.pass / executed,
      complete:
        executed === 20 && counts.blocked === 0 && counts["not-run"] === 0,
      failureStepCounts: failures,
    };
  });
  return {
    schemaVersion: 1,
    transport: campaign.transport,
    nativeWindows: "deferred",
    complete: targets.every((target) => target.complete),
    targets,
  };
}

export function classifyPreflight({
  runningKernel,
  moduleKernels,
  containerState,
  startError,
}) {
  if (
    startError.includes("pair interfaces: operation not supported") &&
    !moduleKernels.includes(runningKernel)
  ) {
    return "host-kernel-modules-mismatch";
  }
  if (containerState !== "running") return "guest-not-running";
  return "requires-guest-interactive-verification";
}
