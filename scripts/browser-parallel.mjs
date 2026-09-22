// Bounded, opt-in parallel scheduling for one declarative browser suite (#155).
//
// Running two CLI processes against one prepared WinBoat Runtime cannot make
// the shared VM lifecycle, the shared build, or the shared server data safe by
// itself. Parallelism therefore stays inside one runner process, where the
// suite declares the resource each test needs and this scheduler enforces the
// conflict policy. Nothing here imports Playwright or touches the filesystem,
// so the policy is unit-testable without a browser.

export const MAX_BROWSER_WORKERS = 8;
export const DEFAULT_WORKER_TEST_TIMEOUT_MILLISECONDS = 600_000;
export const MINIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS = 1_000;
export const MAXIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS = 1_800_000;
/// A Chromium worker plus its contexts is budgeted before more lanes start.
export const WORKER_MEMORY_BUDGET_BYTES = 768 * 1024 * 1024;

export const RESOURCE_KINDS = Object.freeze([
  "app-read",
  "data-write",
  "studio-ui",
  "vm-lifecycle",
]);
export const SESSION_ROLES = Object.freeze(["owner", "participant"]);
export const INVALIDATION_REASONS = Object.freeze([
  "environment-change",
  "timeout",
  "cancelled",
]);

const SCOPE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

export class ConcurrencyError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "ConcurrencyError";
    this.code = code;
  }
}

function invalidSuite(message) {
  return new ConcurrencyError("invalid_suite", message);
}

function invalidRequest(message) {
  return new ConcurrencyError("invalid_request", message);
}

function isPlainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertKeys(value, allowed, message) {
  for (const key of Object.keys(value)) {
    if (!allowed.includes(key)) throw invalidSuite(`${message}: ${key}`);
  }
}

function integerWithin(value, minimum, maximum) {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= minimum &&
    value <= maximum
  );
}

/// The requested worker budget. Absent concurrency keeps the historical
/// single-worker behavior byte for byte: one lane, no per-test deadline.
export function normalizeConcurrencyPolicy(value) {
  if (value === undefined || value === null) {
    return { workers: 1, testTimeoutMilliseconds: null };
  }
  if (!isPlainObject(value)) {
    throw invalidRequest("invalid browser concurrency policy");
  }
  for (const key of Object.keys(value)) {
    if (!["workers", "testTimeoutMilliseconds"].includes(key)) {
      throw invalidRequest(`invalid browser concurrency policy: ${key}`);
    }
  }
  if (!integerWithin(value.workers, 1, MAX_BROWSER_WORKERS)) {
    throw invalidRequest("invalid browser worker count");
  }
  let testTimeoutMilliseconds = null;
  if (
    value.testTimeoutMilliseconds !== undefined &&
    value.testTimeoutMilliseconds !== null
  ) {
    if (
      !integerWithin(
        value.testTimeoutMilliseconds,
        MINIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS,
        MAXIMUM_WORKER_TEST_TIMEOUT_MILLISECONDS,
      )
    ) {
      throw invalidRequest("invalid browser worker test timeout");
    }
    testTimeoutMilliseconds = value.testTimeoutMilliseconds;
  } else if (value.workers > 1) {
    testTimeoutMilliseconds = DEFAULT_WORKER_TEST_TIMEOUT_MILLISECONDS;
  }
  return { workers: value.workers, testTimeoutMilliseconds };
}

export function normalizeSessionRole(value) {
  if (value === undefined || value === null) return "owner";
  if (!SESSION_ROLES.includes(value)) {
    throw invalidRequest("invalid browser session role");
  }
  return value;
}

/// One test's declared resource need. An undeclared test is never assumed to
/// be parallel-safe: it joins the serial data group, so an old suite gains no
/// unsafe overlap merely because the operator asked for workers.
export function normalizeTestConcurrency(
  value,
  { sessionRole = "owner" } = {},
) {
  if (value === undefined) {
    return {
      resource: "data-write",
      scope: null,
      isolation: "unverified",
      declared: false,
    };
  }
  if (!isPlainObject(value)) throw invalidSuite("invalid test concurrency");
  assertKeys(
    value,
    ["resource", "scope", "isolation"],
    "invalid test concurrency",
  );
  const { resource } = value;
  if (!RESOURCE_KINDS.includes(resource)) {
    throw invalidSuite("invalid test concurrency resource");
  }
  if (value.scope !== undefined) {
    if (typeof value.scope !== "string" || !SCOPE_PATTERN.test(value.scope)) {
      throw invalidSuite("invalid test concurrency scope");
    }
    if (resource !== "data-write" && resource !== "studio-ui") {
      throw invalidSuite(
        "a concurrency scope applies only to data-write and studio-ui tests",
      );
    }
  }
  let isolation = "unverified";
  if (value.isolation !== undefined) {
    if (value.isolation !== "verified" && value.isolation !== "unverified") {
      throw invalidSuite("invalid test concurrency isolation");
    }
    isolation = value.isolation;
  }
  // Verified isolation is a claim about data, and it is only meaningful when
  // the suite names the isolated scope. Write-back and import/export tests
  // without that proof are refused as "verified" and run serially instead.
  if (isolation === "verified") {
    if (resource !== "data-write") {
      throw invalidSuite(
        "only a data-write test can declare verified data isolation",
      );
    }
    if (value.scope === undefined) {
      throw invalidSuite(
        "verified data isolation requires an explicit data scope",
      );
    }
  }
  // An attached participant inherits a prepared Runtime; the owner prepares
  // and cleans up. A participant therefore never runs VM lifecycle work.
  if (resource === "vm-lifecycle" && sessionRole === "participant") {
    throw new ConcurrencyError(
      "concurrency_policy_refused",
      "a shared-session participant must not run a vm-lifecycle test",
    );
  }
  return {
    resource,
    scope: value.scope ?? null,
    isolation,
    declared: true,
  };
}

/// How many tests of this suite could ever hold locks at the same time.
export function declaredParallelCapacity(plans) {
  let capacity = 0;
  const scopes = new Set();
  let studio = false;
  for (const plan of plans) {
    if (plan.resource === "app-read") capacity += 1;
    else if (plan.resource === "data-write" && plan.isolation === "verified") {
      scopes.add(plan.scope);
    } else if (plan.resource === "studio-ui") studio = true;
  }
  capacity += scopes.size + (studio ? 1 : 0);
  return Math.max(1, capacity);
}

/// Bound the requested lanes by the host's CPU and memory headroom and by what
/// the suite could actually overlap, and report which limit applied.
export function resolveWorkerLimit({
  requested,
  capacity = Number.MAX_SAFE_INTEGER,
  testCount = Number.MAX_SAFE_INTEGER,
  cpuCount = 1,
  availableMemoryBytes = Number.MAX_SAFE_INTEGER,
  memoryBudgetBytes = WORKER_MEMORY_BUDGET_BYTES,
}) {
  let workers = Math.min(Math.max(1, requested), MAX_BROWSER_WORKERS);
  let limitedBy = "request";
  const cpuLimit = Math.max(1, Math.floor(cpuCount) || 1);
  if (cpuLimit < workers) {
    workers = cpuLimit;
    limitedBy = "cpu";
  }
  const memoryLimit = Math.max(
    1,
    Math.floor(availableMemoryBytes / Math.max(1, memoryBudgetBytes)),
  );
  if (memoryLimit < workers) {
    workers = memoryLimit;
    limitedBy = "memory";
  }
  const suiteLimit = Math.max(1, Math.min(capacity, testCount));
  if (suiteLimit < workers) {
    workers = suiteLimit;
    limitedBy = "suite";
  }
  return { workers, limitedBy };
}

/// The deterministic parallel-permission table published with every run.
export function describeConcurrencyGroups(plans) {
  const buckets = new Map();
  const record = (resource, mode, scope) => {
    const key = `${resource}/${mode}`;
    const bucket = buckets.get(key) ?? {
      resource,
      mode,
      tests: 0,
      scopes: new Set(),
    };
    bucket.tests += 1;
    if (scope) bucket.scopes.add(scope);
    buckets.set(key, bucket);
  };
  for (const plan of plans) {
    if (plan.resource === "app-read") record("app-read", "parallel");
    else if (plan.resource === "vm-lifecycle") {
      record("vm-lifecycle", "exclusive");
    } else if (plan.resource === "studio-ui") record("studio-ui", "serial");
    else if (plan.isolation === "verified") {
      record("data-write", "scoped-parallel", plan.scope);
    } else record("data-write", "serial");
  }
  const order = [
    "app-read/parallel",
    "data-write/scoped-parallel",
    "data-write/serial",
    "studio-ui/serial",
    "vm-lifecycle/exclusive",
  ];
  return order
    .filter((key) => buckets.has(key))
    .map((key) => {
      const bucket = buckets.get(key);
      return {
        resource: bucket.resource,
        mode: bucket.mode,
        tests: bucket.tests,
        ...(bucket.mode === "scoped-parallel"
          ? { scopes: bucket.scopes.size }
          : {}),
      };
    });
}

/// Readers/writer arbitration over the shared app data, the single Studio UI
/// turn, and the exclusive VM lifecycle barrier. A test acquires its complete
/// lock set atomically before it starts, so no lane can deadlock mid-test.
export class ResourceScheduler {
  constructor() {
    this.dataReaders = 0;
    this.dataWriter = false;
    this.scopes = new Set();
    this.studio = false;
    this.exclusive = false;
    this.active = 0;
  }

  get inFlight() {
    return this.active;
  }

  admits(plan) {
    if (this.exclusive) return false;
    if (plan.resource === "vm-lifecycle") return this.active === 0;
    if (this.dataWriter) return false;
    if (plan.resource === "studio-ui") return !this.studio;
    if (plan.resource === "app-read") return true;
    if (plan.isolation === "verified") return !this.scopes.has(plan.scope);
    // An unverified write may hold no reader beside it: a concurrent "stable
    // app read" or another write could observe or destroy its data.
    return this.dataReaders === 0 && !this.studio;
  }

  acquire(plan) {
    if (!this.admits(plan)) return false;
    this.active += 1;
    if (plan.resource === "vm-lifecycle") {
      this.exclusive = true;
      return true;
    }
    if (plan.resource === "studio-ui") {
      this.studio = true;
      this.dataReaders += 1;
      return true;
    }
    if (plan.resource === "app-read") {
      this.dataReaders += 1;
      return true;
    }
    if (plan.isolation === "verified") {
      this.scopes.add(plan.scope);
      this.dataReaders += 1;
      return true;
    }
    this.dataWriter = true;
    return true;
  }

  release(plan) {
    this.active -= 1;
    if (plan.resource === "vm-lifecycle") {
      this.exclusive = false;
      return;
    }
    if (plan.resource === "studio-ui") {
      this.studio = false;
      this.dataReaders -= 1;
      return;
    }
    if (plan.resource === "app-read") {
      this.dataReaders -= 1;
      return;
    }
    if (plan.isolation === "verified") {
      this.scopes.delete(plan.scope);
      this.dataReaders -= 1;
      return;
    }
    this.dataWriter = false;
  }
}

/// Abort-aware wrapper: the work keeps running inside Playwright, but the lane
/// stops waiting for it and the caller tears its own context down.
export function cancellable(signal, work) {
  if (!signal) return work();
  if (signal.aborted) return Promise.reject(abortError(signal));
  return new Promise((resolve, reject) => {
    const onAbort = () => reject(abortError(signal));
    signal.addEventListener("abort", onAbort, { once: true });
    Promise.resolve()
      .then(work)
      .then(resolve, reject)
      .finally(() => signal.removeEventListener("abort", onAbort));
  });
}

function abortError(signal) {
  const reason = INVALIDATION_REASONS.includes(signal.reason)
    ? signal.reason
    : "cancelled";
  const error = new Error(
    reason === "timeout"
      ? "the worker test timeout elapsed before this test finished"
      : reason === "environment-change"
        ? "environment observation interrupted this test; see environment.events"
        : "this test was cancelled",
  );
  error.invalidatedBy = reason;
  return error;
}

/// Run the suite with at most `workers` lanes.
///
/// - Admission is head-of-line in declaration order, so scheduling is
///   reproducible for a given set of durations and never starves an exclusive
///   test behind a stream of readers.
/// - Results are returned in declaration order regardless of completion order.
/// - One lane's failure, timeout, or crash resolves only that entry; the other
///   lanes keep their browser, context, and artifacts.
export async function runBounded({
  plans,
  workers,
  execute,
  skip,
  isInterrupted = () => false,
  testTimeoutMilliseconds = null,
  interruptionPollMilliseconds = 250,
  scheduleTimeout = (handler, milliseconds) =>
    globalThis.setTimeout(handler, milliseconds),
  clearScheduledTimeout = (handle) => globalThis.clearTimeout(handle),
}) {
  const lanes = [];
  for (let lane = workers - 1; lane >= 0; lane -= 1) lanes.push(lane);
  const pending = plans.map((_, index) => index);
  const outcomes = new Array(plans.length);
  const scheduler = new ResourceScheduler();
  const running = new Map();
  let maxObservedParallel = 0;
  let interruptedAt = null;
  const poll = Symbol("poll");

  const drainPending = (reason) => {
    while (pending.length > 0) {
      const index = pending.shift();
      outcomes[index] = skip(index, reason);
    }
  };

  const start = (index, lane) => {
    const plan = plans[index];
    const controller = new AbortController();
    const timer =
      testTimeoutMilliseconds === null
        ? null
        : scheduleTimeout(
            () => controller.abort("timeout"),
            testTimeoutMilliseconds,
          );
    const finished = (async () => {
      try {
        return { index, value: await execute(index, controller.signal, lane) };
      } catch (error) {
        return { index, error: error instanceof Error ? error : new Error() };
      } finally {
        if (timer !== null) clearScheduledTimeout(timer);
      }
    })();
    running.set(index, { plan, lane, controller, finished });
    maxObservedParallel = Math.max(maxObservedParallel, running.size);
  };

  const admit = () => {
    while (lanes.length > 0) {
      let started = false;
      for (let position = 0; position < pending.length; position += 1) {
        const index = pending[position];
        const plan = plans[index];
        if (scheduler.acquire(plan)) {
          pending.splice(position, 1);
          start(index, lanes.pop());
          started = true;
          break;
        }
        // An exclusive test is a barrier: later tests never overtake it, so a
        // VM lifecycle test cannot be postponed forever by shorter readers.
        if (plan.resource === "vm-lifecycle") return;
      }
      if (!started) return;
    }
  };

  while (pending.length > 0 || running.size > 0) {
    if (isInterrupted() && interruptedAt === null) {
      interruptedAt = Date.now();
      drainPending("environment-change");
      for (const entry of running.values()) {
        entry.controller.abort("environment-change");
      }
    }
    if (interruptedAt === null) admit();
    if (running.size === 0) {
      if (pending.length === 0) break;
      // Unreachable while every lock set is acquired atomically; refuse to
      // spin instead of hanging if that invariant is ever broken.
      drainPending("cancelled");
      break;
    }
    const waiters = [...running.values()].map((entry) => entry.finished);
    let pollTimer = null;
    if (interruptedAt === null) {
      // A change observed while every lane is busy must not wait for the
      // slowest lane: the loop wakes up and cancels the in-flight tests.
      waiters.push(
        new Promise((resolve) => {
          pollTimer = scheduleTimeout(
            () => resolve(poll),
            interruptionPollMilliseconds,
          );
          pollTimer?.unref?.();
        }),
      );
    }
    const settled = await Promise.race(waiters);
    if (pollTimer !== null) clearScheduledTimeout(pollTimer);
    if (settled === poll) continue;
    const entry = running.get(settled.index);
    running.delete(settled.index);
    scheduler.release(entry.plan);
    lanes.push(entry.lane);
    outcomes[settled.index] = settled.error
      ? { failure: settled.error }
      : { value: settled.value };
  }

  return { outcomes, maxObservedParallel };
}
