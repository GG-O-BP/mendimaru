import assert from "node:assert/strict";
import { test } from "node:test";
import {
  assertRun,
  validateReadSuite,
  runSharedGate,
} from "./test-shared-winboat-live.mjs";
const suite = () => ({
  schemaVersion: "1.0.0",
  tests: [1, 2].map((index) => ({
    name: `app ${index}`,
    concurrency: { resource: "app-read" },
    steps: [
      { action: "expectVisible", locator: { by: "testId", value: "app" } },
    ],
  })),
});
const result = () => ({
  data: {
    outcome: "passed",
    passed: 2,
    failed: 0,
    browserParity: "unmodified",
    environment: { comparable: true },
    concurrency: { sessionRole: "participant", maxObservedParallel: 2 },
    corrections: [{ applied: false }],
  },
});
test("live gate requires independent asserted app reads", () => {
  validateReadSuite(suite());
  for (const resource of [
    undefined,
    "data-write",
    "studio-ui",
    "vm-lifecycle",
  ]) {
    const changed = suite();
    changed.tests[0].concurrency.resource = resource;
    assert.throws(() => validateReadSuite(changed));
  }
  const changed = suite();
  changed.tests[1].steps = [{ action: "goto", path: "/" }];
  assert.throws(() => validateReadSuite(changed));
  assert.throws(() =>
    validateReadSuite({ ...suite(), tests: [suite().tests[0]] }),
  );
});
test("live acceptance rejects mirrored, incomparable, serial, or empty passes", () => {
  assertRun(result(), { parallel: true });
  for (const mutate of [
    (r) => {
      r.data.browserParity = "assisted";
    },
    (r) => {
      r.data.environment.comparable = false;
    },
    (r) => {
      r.data.concurrency.maxObservedParallel = 1;
    },
    (r) => {
      r.data.concurrency.sessionRole = "owner";
    },
    (r) => {
      r.data.passed = 0;
    },
    (r) => {
      r.data.corrections[0].applied = true;
    },
  ]) {
    const changed = result();
    mutate(changed);
    assert.throws(() => assertRun(changed, { parallel: true }));
  }
});
test("no mutation opt-in fails before any VM or CLI access", async () => {
  const old = process.env.MENDIMARU_E2E_ALLOW_MUTATION;
  delete process.env.MENDIMARU_E2E_ALLOW_MUTATION;
  try {
    await assert.rejects(runSharedGate());
  } finally {
    if (old !== undefined) process.env.MENDIMARU_E2E_ALLOW_MUTATION = old;
  }
});
