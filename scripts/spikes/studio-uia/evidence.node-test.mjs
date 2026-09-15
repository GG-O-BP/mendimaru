import assert from "node:assert/strict";
import test from "node:test";
import { classifyPreflight, steps, summarize } from "./evidence.mjs";

function fixture(status = "pass") {
  return {
    schemaVersion: 1,
    transport: "linux-winboat",
    nativeWindows: "deferred",
    targets: ["10.24.0", "11.12.0"].map((version) => ({
      version,
      projectSha256: "a".repeat(64),
      trials: Array.from({ length: 20 }, (_, index) => ({
        id: index + 1,
        phase: index < 5 ? "cold" : "warm",
        steps: steps.map((id) => ({
          id,
          status,
          elapsedMs: status === "pass" ? 12 : 0,
          effectVerified: status === "pass",
          artifacts:
            status === "pass" ? [`${version}/${index + 1}/${id}.json`] : [],
          reason: status === "pass" ? undefined : "guest-not-running",
        })),
      })),
    })),
  };
}

test("complete measured campaigns preserve cold and warm denominators", () => {
  const report = summarize(fixture());
  assert.equal(report.complete, true);
  assert.equal(report.nativeWindows, "deferred");
  assert.equal(report.targets[0].successRate, 1);
  assert.equal(report.targets[0].phaseCounts.cold.passed, 5);
  assert.equal(report.targets[0].phaseCounts.warm.passed, 15);
});

test("missing VM produces no UI success rate, never a measured No-Go", () => {
  const report = summarize(fixture("blocked"));
  assert.equal(report.complete, false);
  assert.equal(report.targets[0].executed, 0);
  assert.equal(report.targets[0].successRate, null);
  assert.equal(report.targets[0].counts.blocked, 20);
});

test("partial and failed flows count in the executed denominator", () => {
  const campaign = fixture();
  campaign.targets[0].trials[0].steps[2] = {
    id: "change-property",
    status: "blocked",
    elapsedMs: 100,
    artifacts: [],
    reason: "no-semantic-locator",
  };
  campaign.targets[0].trials[1].steps[2].status = "fail";
  campaign.targets[0].trials[1].steps[2].reason = "readback-mismatch";
  const report = summarize(campaign).targets[0];
  assert.equal(report.executed, 20);
  assert.equal(report.successRate, 0.9);
  assert.equal(report.complete, false);
  assert.deepEqual(report.counts, {
    pass: 18,
    fail: 1,
    blocked: 1,
    "not-run": 0,
  });
});

test("rejects duplicate trials, omitted actions, unverified effects and unsafe paths", () => {
  for (const corrupt of [
    (c) => c.targets[0].trials.pop(),
    (c) => {
      c.targets[0].trials[1].id = 1;
    },
    (c) => c.targets[0].trials[0].steps.pop(),
    (c) => {
      c.targets[0].trials[0].steps[0].effectVerified = false;
    },
    (c) => {
      c.targets[0].trials[0].steps[0].artifacts = [];
    },
    (c) => {
      c.targets[0].trials[0].steps[0].artifacts = ["../private.json"];
    },
    (c) => {
      c.targets[0].trials[0].steps[0].elapsedMs = Infinity;
    },
    (c) => {
      c.targets[0].version = "11.13.0";
    },
    (c) => {
      c.transport = "windows-native";
    },
    (c) => {
      c.targets[0].trials.forEach((t) => {
        t.phase = "warm";
      });
    },
  ]) {
    const campaign = fixture();
    corrupt(campaign);
    assert.throws(() => summarize(campaign));
  }
});

test("kernel mismatch needs both the exact observed network failure and absent modules", () => {
  const facts = {
    runningKernel: "7.2.4-arch1-2",
    moduleKernels: ["7.2.6-arch2-1"],
    containerState: "exited",
    startError:
      "failed to add the host (vethX) <=> sandbox (vethY) pair interfaces: operation not supported",
  };
  assert.equal(classifyPreflight(facts), "host-kernel-modules-mismatch");
  assert.equal(
    classifyPreflight({ ...facts, startError: "another failure" }),
    "guest-not-running",
  );
  assert.equal(
    classifyPreflight({ ...facts, moduleKernels: [facts.runningKernel] }),
    "guest-not-running",
  );
  assert.equal(
    classifyPreflight({ ...facts, containerState: "running", startError: "" }),
    "requires-guest-interactive-verification",
  );
});
