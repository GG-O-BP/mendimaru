import assert from "node:assert/strict";
import test from "node:test";
import {
  ConcurrencyError,
  DEFAULT_WORKER_TEST_TIMEOUT_MILLISECONDS,
  MAX_BROWSER_WORKERS,
  ResourceScheduler,
  cancellable,
  declaredParallelCapacity,
  describeConcurrencyGroups,
  normalizeConcurrencyPolicy,
  normalizeSessionRole,
  normalizeTestConcurrency,
  resolveWorkerLimit,
  runBounded,
} from "./browser-parallel.mjs";

const appRead = { resource: "app-read" };
const studioUi = { resource: "studio-ui" };
const lifecycle = { resource: "vm-lifecycle" };
const isolatedWrite = (scope) => ({
  resource: "data-write",
  scope,
  isolation: "verified",
});

function plan(value, sessionRole = "owner") {
  return normalizeTestConcurrency(value, { sessionRole });
}

function refuses(value, sessionRole = "owner") {
  try {
    plan(value, sessionRole);
  } catch (error) {
    assert.equal(error instanceof ConcurrencyError, true);
    return error;
  }
  return assert.fail(`accepted ${JSON.stringify(value)}`);
}

test("an absent concurrency policy keeps the historical single worker", () => {
  assert.deepEqual(normalizeConcurrencyPolicy(undefined), {
    workers: 1,
    testTimeoutMilliseconds: null,
  });
  assert.deepEqual(normalizeConcurrencyPolicy(null), {
    workers: 1,
    testTimeoutMilliseconds: null,
  });
  assert.deepEqual(normalizeConcurrencyPolicy({ workers: 1 }), {
    workers: 1,
    testTimeoutMilliseconds: null,
  });
});

test("more than one worker gets a default per-test deadline", () => {
  assert.deepEqual(normalizeConcurrencyPolicy({ workers: 3 }), {
    workers: 3,
    testTimeoutMilliseconds: DEFAULT_WORKER_TEST_TIMEOUT_MILLISECONDS,
  });
  assert.deepEqual(
    normalizeConcurrencyPolicy({ workers: 2, testTimeoutMilliseconds: 5_000 }),
    { workers: 2, testTimeoutMilliseconds: 5_000 },
  );
});

test("the worker policy is bounded and closed", () => {
  for (const value of [
    { workers: 0 },
    { workers: MAX_BROWSER_WORKERS + 1 },
    { workers: 1.5 },
    { workers: "2" },
    { workers: 2, testTimeoutMilliseconds: 10 },
    { workers: 2, testTimeoutMilliseconds: 1_800_001 },
    { workers: 2, unknown: true },
    [],
  ]) {
    assert.throws(
      () => normalizeConcurrencyPolicy(value),
      (error) => error.code === "invalid_request",
      `accepted ${JSON.stringify(value)}`,
    );
  }
});

test("an undeclared test is never assumed to be parallel-safe", () => {
  assert.deepEqual(plan(undefined), {
    resource: "data-write",
    scope: null,
    isolation: "unverified",
    declared: false,
  });
  const scheduler = new ResourceScheduler();
  assert.equal(scheduler.acquire(plan(undefined)), true);
  assert.equal(scheduler.admits(plan(undefined)), false);
  assert.equal(scheduler.admits(plan(appRead)), false);
});

test("resource declarations are validated and closed", () => {
  refuses({ resource: "anything" });
  refuses({ resource: "app-read", scope: "orders" });
  refuses({ resource: "vm-lifecycle", scope: "orders" });
  refuses({ resource: "data-write", isolation: "verified" });
  refuses({ resource: "studio-ui", isolation: "verified", scope: "f5" });
  refuses({ resource: "data-write", scope: "not a scope" });
  refuses({ resource: "data-write", scope: "x".repeat(65) });
  refuses({ resource: "data-write", scope: "orders", extra: 1 });
  refuses("app-read");
  assert.deepEqual(plan(isolatedWrite("orders-a")), {
    resource: "data-write",
    scope: "orders-a",
    isolation: "verified",
    declared: true,
  });
});

test("an attached participant may not run a vm-lifecycle test", () => {
  assert.equal(
    refuses(lifecycle, "participant").code,
    "concurrency_policy_refused",
  );
  assert.equal(refuses({ resource: "anything" }).code, "invalid_suite");
  assert.equal(plan(lifecycle, "owner").resource, "vm-lifecycle");
  assert.equal(plan(appRead, "participant").resource, "app-read");
  assert.equal(normalizeSessionRole(undefined), "owner");
  assert.throws(() => normalizeSessionRole("owner "));
});

test("stable app reads share the app while an unproven write excludes it", () => {
  const scheduler = new ResourceScheduler();
  assert.equal(scheduler.acquire(plan(appRead)), true);
  assert.equal(scheduler.acquire(plan(appRead)), true);
  // An unverified write-back or import/export never runs beside a reader.
  assert.equal(scheduler.admits(plan({ resource: "data-write" })), false);
  assert.equal(scheduler.acquire(plan(isolatedWrite("orders-a"))), true);
  assert.equal(scheduler.acquire(plan(isolatedWrite("orders-b"))), true);
  assert.equal(scheduler.admits(plan(isolatedWrite("orders-a"))), false);
  scheduler.release(plan(isolatedWrite("orders-a")));
  assert.equal(scheduler.admits(plan(isolatedWrite("orders-a"))), true);
});

test("one Studio UI action runs at a time and excludes VM lifecycle work", () => {
  const scheduler = new ResourceScheduler();
  assert.equal(scheduler.acquire(plan(studioUi)), true);
  assert.equal(scheduler.admits(plan(studioUi)), false);
  assert.equal(
    scheduler.admits(plan({ resource: "studio-ui", scope: "f5" })),
    false,
  );
  assert.equal(scheduler.admits(plan(appRead)), true);
  assert.equal(scheduler.admits(plan(lifecycle)), false);
  scheduler.release(plan(studioUi));
  assert.equal(scheduler.acquire(plan(lifecycle)), true);
  for (const other of [appRead, studioUi, lifecycle, isolatedWrite("a")]) {
    assert.equal(scheduler.admits(plan(other)), false);
  }
});

test("the declared capacity and the host bound the worker count", () => {
  const plans = [appRead, appRead, isolatedWrite("a"), studioUi].map((value) =>
    plan(value),
  );
  assert.equal(declaredParallelCapacity(plans), 4);
  assert.equal(
    declaredParallelCapacity([
      plan(undefined),
      plan({ resource: "data-write" }),
    ]),
    1,
  );
  assert.deepEqual(
    resolveWorkerLimit({
      requested: 4,
      capacity: 4,
      testCount: 10,
      cpuCount: 16,
      availableMemoryBytes: 32 * 1024 ** 3,
    }),
    { workers: 4, limitedBy: "request" },
  );
  assert.deepEqual(
    resolveWorkerLimit({
      requested: 8,
      capacity: 8,
      testCount: 10,
      cpuCount: 2,
      availableMemoryBytes: 32 * 1024 ** 3,
    }),
    { workers: 2, limitedBy: "cpu" },
  );
  assert.deepEqual(
    resolveWorkerLimit({
      requested: 8,
      capacity: 8,
      testCount: 10,
      cpuCount: 16,
      availableMemoryBytes: 1024 ** 3,
      memoryBudgetBytes: 768 * 1024 ** 2,
    }),
    { workers: 1, limitedBy: "memory" },
  );
  assert.deepEqual(
    resolveWorkerLimit({
      requested: 8,
      capacity: 1,
      testCount: 10,
      cpuCount: 16,
      availableMemoryBytes: 32 * 1024 ** 3,
    }),
    { workers: 1, limitedBy: "suite" },
  );
  assert.deepEqual(
    resolveWorkerLimit({ requested: 1, cpuCount: 1, availableMemoryBytes: 0 }),
    { workers: 1, limitedBy: "request" },
  );
});

test("the parallel permission table is deterministic", () => {
  const plans = [
    lifecycle,
    { resource: "data-write" },
    appRead,
    isolatedWrite("b"),
    studioUi,
    isolatedWrite("a"),
    undefined,
  ].map((value) => plan(value));
  assert.deepEqual(describeConcurrencyGroups(plans), [
    { resource: "app-read", mode: "parallel", tests: 1 },
    { resource: "data-write", mode: "scoped-parallel", tests: 2, scopes: 2 },
    { resource: "data-write", mode: "serial", tests: 2 },
    { resource: "studio-ui", mode: "serial", tests: 1 },
    { resource: "vm-lifecycle", mode: "exclusive", tests: 1 },
  ]);
});

function recordingExecutor({ durations = {}, failures = {} } = {}) {
  const state = { overlap: 0, peak: 0, order: [], finished: [], aborted: [] };
  const execute = async (index, signal) => {
    state.overlap += 1;
    state.peak = Math.max(state.peak, state.overlap);
    state.order.push(index);
    try {
      await cancellable(
        signal,
        () =>
          new Promise((resolve) => setTimeout(resolve, durations[index] ?? 1)),
      );
    } catch (error) {
      state.aborted.push(index);
      throw error;
    } finally {
      state.overlap -= 1;
    }
    if (failures[index]) throw new Error(failures[index]);
    state.finished.push(index);
    return `value-${index}`;
  };
  return { state, execute };
}

const skip = (index, reason) => ({ value: `skipped-${index}-${reason}` });

test("one worker keeps the historical sequential order", async () => {
  const plans = [appRead, appRead, appRead].map((value) => plan(value));
  const { state, execute } = recordingExecutor({ durations: { 0: 12, 1: 1 } });
  const { outcomes, maxObservedParallel } = await runBounded({
    plans,
    workers: 1,
    execute,
    skip,
  });
  assert.equal(maxObservedParallel, 1);
  assert.equal(state.peak, 1);
  assert.deepEqual(state.order, [0, 1, 2]);
  assert.deepEqual(
    outcomes.map((outcome) => outcome.value),
    ["value-0", "value-1", "value-2"],
  );
});

test("declared readers overlap and results stay in declaration order", async () => {
  const plans = [appRead, appRead, appRead, appRead].map((value) =>
    plan(value),
  );
  const { state, execute } = recordingExecutor({
    durations: { 0: 40, 1: 5, 2: 5, 3: 5 },
  });
  const { outcomes, maxObservedParallel } = await runBounded({
    plans,
    workers: 3,
    execute,
    skip,
  });
  assert.equal(maxObservedParallel, 3, "three lanes must overlap");
  assert.equal(state.peak, 3);
  assert.notDeepEqual(state.finished, [0, 1, 2, 3]);
  assert.deepEqual(
    outcomes.map((outcome) => outcome.value),
    ["value-0", "value-1", "value-2", "value-3"],
  );
});

test("an exclusive test is a barrier that later tests never overtake", async () => {
  const plans = [appRead, lifecycle, appRead, appRead].map((value) =>
    plan(value),
  );
  const { state, execute } = recordingExecutor({ durations: { 0: 10, 1: 5 } });
  await runBounded({ plans, workers: 4, execute, skip });
  assert.deepEqual(state.order.slice(0, 2), [0, 1]);
  assert.deepEqual(state.order.slice(2).sort(), [2, 3]);
  // Nothing ran while the exclusive test held the barrier.
  assert.equal(state.peak <= 2, true);
});

test("unproven writes serialize while isolated scopes overlap", async () => {
  const serial = [
    { resource: "data-write" },
    { resource: "data-write" },
    { resource: "data-write" },
  ].map((value) => plan(value));
  const serialRun = recordingExecutor({ durations: { 0: 8, 1: 8, 2: 8 } });
  const serialResult = await runBounded({
    plans: serial,
    workers: 3,
    execute: serialRun.execute,
    skip,
  });
  assert.equal(serialResult.maxObservedParallel, 1);
  assert.deepEqual(serialRun.state.order, [0, 1, 2]);

  const isolated = [
    isolatedWrite("a"),
    isolatedWrite("b"),
    isolatedWrite("a"),
  ].map((value) => plan(value));
  const isolatedRun = recordingExecutor({ durations: { 0: 25, 1: 5, 2: 5 } });
  const isolatedResult = await runBounded({
    plans: isolated,
    workers: 3,
    execute: isolatedRun.execute,
    skip,
  });
  assert.equal(isolatedResult.maxObservedParallel, 2);
  assert.deepEqual(isolatedRun.state.order, [0, 1, 2]);
  assert.equal(isolatedRun.state.finished.at(-1), 2);
});

test("one worker failure or crash never cancels the other workers", async () => {
  const plans = [appRead, appRead, appRead].map((value) => plan(value));
  const { state, execute } = recordingExecutor({
    durations: { 0: 2, 1: 20, 2: 20 },
    failures: { 0: "worker crashed" },
  });
  const { outcomes } = await runBounded({ plans, workers: 3, execute, skip });
  assert.equal(outcomes[0].failure.message, "worker crashed");
  assert.equal(outcomes[1].value, "value-1");
  assert.equal(outcomes[2].value, "value-2");
  assert.deepEqual(state.finished.sort(), [1, 2]);
  assert.deepEqual(state.aborted, []);
});

test("a worker deadline cancels only its own test", async () => {
  const plans = [appRead, appRead].map((value) => plan(value));
  const { state, execute } = recordingExecutor({
    durations: { 0: 5_000, 1: 5 },
  });
  const { outcomes } = await runBounded({
    plans,
    workers: 2,
    execute,
    skip,
    testTimeoutMilliseconds: 30,
  });
  assert.equal(outcomes[0].failure.invalidatedBy, "timeout");
  assert.equal(outcomes[1].value, "value-1");
  assert.deepEqual(state.aborted, [0]);
  assert.deepEqual(state.finished, [1]);
});

test("an environment change invalidates running and pending tests", async () => {
  const plans = [appRead, appRead, appRead, appRead].map((value) =>
    plan(value),
  );
  let interrupted = false;
  const { state, execute } = recordingExecutor({
    durations: { 0: 5_000, 1: 5_000 },
  });
  const wrapped = async (index, signal, lane) => {
    if (index === 1) setTimeout(() => (interrupted = true), 10);
    return execute(index, signal, lane);
  };
  const { outcomes } = await runBounded({
    plans,
    workers: 2,
    execute: wrapped,
    skip,
    isInterrupted: () => interrupted,
    interruptionPollMilliseconds: 5,
  });
  assert.equal(outcomes[0].failure.invalidatedBy, "environment-change");
  assert.equal(outcomes[1].failure.invalidatedBy, "environment-change");
  assert.equal(outcomes[2].value, "skipped-2-environment-change");
  assert.equal(outcomes[3].value, "skipped-3-environment-change");
  assert.deepEqual(state.finished, []);
  assert.deepEqual(state.order, [0, 1]);
});

test("an already interrupted environment starts no test at all", async () => {
  const plans = [appRead, appRead].map((value) => plan(value));
  const { state, execute } = recordingExecutor();
  const { outcomes, maxObservedParallel } = await runBounded({
    plans,
    workers: 2,
    execute,
    skip,
    isInterrupted: () => true,
  });
  assert.equal(maxObservedParallel, 0);
  assert.deepEqual(state.order, []);
  assert.deepEqual(
    outcomes.map((outcome) => outcome.value),
    ["skipped-0-environment-change", "skipped-1-environment-change"],
  );
});

test("cancellation is observable and idempotent", async () => {
  const controller = new AbortController();
  controller.abort("timeout");
  const error = await cancellable(controller.signal, () => {
    throw new Error("never runs");
  }).catch((value) => value);
  assert.equal(error.invalidatedBy, "timeout");
  const unknown = new AbortController();
  unknown.abort("something else");
  const fallback = await cancellable(unknown.signal, () => Promise.resolve(1))
    .then(() => null)
    .catch((value) => value);
  assert.equal(fallback.invalidatedBy, "cancelled");
  assert.equal(await cancellable(undefined, () => 7), 7);
});
