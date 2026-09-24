// Issue #182, cause (2). The measured post-merge envelope is decided by whether
// the installer job's Rust dependency cache survives between two consecutive
// runs. It did not: the repository held 11.27 GB across 27 entries against
// GitHub's 10 GB per-repository cap, so
// `v0-rust-release-performance-windows-bundles-candidate-*` reported
// `No cache found` at 01:17 and was re-saved, 612 MB, at 01:52 the same hour.
// With it missing the build compiled all 395 crates instead of the workspace
// crate and the link step, which is the difference between a 13.30 minute and
// a 20.73 minute envelope.
//
// GitHub's own eviction is least-recently-used across the whole repository, so
// it cannot distinguish a superseded generation of a cache from the entry the
// next run needs; both look equally old until the moment the next run asks for
// one. This plan deletes superseded generations explicitly and keeps headroom
// under the cap so a fresh save never forces GitHub to choose for us.
//
// The module only decides. Deleting is the workflow's job, so the decision
// stays testable and the destructive call stays visible in one place.
import process from "node:process";

export const GIB = 1024 ** 3;

// Standing headroom policy. The effective target also reserves the largest
// observed save under the cap: the previous 9.06 GiB retained floor left only
// 0.94 GiB for a 1.12 GiB save (issue #209).
export const DEFAULT_BUDGET_BYTES = 9 * GIB;

// GitHub's hard per-repository limit. Crossing *this* is what makes GitHub
// evict least-recently-used entries and is the actual cause of #182; the 9 GiB
// budget above is the headroom target we aim for, not the failure condition.
//
// The two have to be separate, because they are not equally reachable. On the
// 2026-09-23 listing the ten `v0-rust-*` dependency families are 7.49 GiB and
// each already holds a single generation, so no prune can shrink them without
// giving up a cache hit - which is a speed regression, the opposite of what
// #182 asks for. With `aur-sccache` (0.89 GiB), Playwright (0.26 GiB) and the
// small fixed entries, the irreducible floor is 9.06 GiB. Pruning takes the
// repository from 11.90 GiB to that floor: over the cap to under it, which is
// the whole objective. Treating the unreachable 9 GiB target as a failure
// would have made this job red on its first run and taught everyone to ignore
// it. So: over the cap is an error, over the budget is a warning.
export const DEFAULT_CAP_BYTES = 10 * GIB;

// A cache being restored right now must not be deleted underneath the run that
// is reading it. GitHub refreshes `last_accessed_at` on restore, so an idle
// window is the available signal. Fifteen minutes covers the download of the
// largest entry here (~1.2 GiB) with room to spare, and staying short matters:
// in a repository whose caches are touched by every run, a long window would
// defer every deletion forever and the prune would never reclaim anything.
export const DEFAULT_IDLE_MINUTES = 15;

// How many generations of a family stay. The distinction is how the entry is
// looked up, not how big it is:
//
//   - Prefix-restored families (`actions/cache` `restore-keys`, and
//     `Swatinem/rust-cache`, which falls back to the newest prefix match)
//     never read an older generation. Keeping one keeps everything that can
//     still be restored.
//   - Fingerprint-keyed families are restored by exact key only, and #178
//     deliberately reuses an older revision's entry when the fingerprint
//     repeats. Baseline and candidate are two live fingerprints per platform
//     at any time, so keeping only one would delete a live entry on every
//     merge. These entries are small (92 MB and 25 MB), so history is cheap.
export const DEFAULT_RETENTION = [
  { prefix: "v0-rust-", keep: 1 },
  { prefix: "aur-sccache-", keep: 1 },
  { prefix: "webview-binary-", keep: 4 },
  { prefix: "bundle-installers", keep: 4 },
];

// Anything unrecognised keeps two generations: enough that a family whose
// restore semantics nobody has classified is not pruned to a single entry by
// accident, few enough that it still stops growing.
export const DEFAULT_KEEP = 2;

// Families no job writes any more. Issue #209 needed the irreducible floor
// below 8.88 GiB, and the way it got there was to point two jobs that compile
// the identical crate graph in the identical directory at one cache key. The
// side effect of every such consolidation is an orphan: the retired family's
// last generation is still in the listing, is still the only generation of its
// family, and is therefore still "retained" - so the retention and budget
// passes both protect bytes that nothing can ever restore. GitHub removes an
// unused cache after seven days on its own, which would leave 0.81 GiB of the
// headroom this change exists to create parked for a week.
//
// Naming the retirement reclaims it on the next prune instead. It is a list
// rather than an inference because "no job writes this any more" is not
// visible from the listing: a relevance-gated job that has not been triggered
// for a while is indistinguishable from a family nobody writes, and guessing
// wrong costs exactly the cold rebuild #182 measured.
//
//   - `v0-rust-test-ubuntu-`: retired 2026-09-23. `Test (ubuntu-latest)` now
//     restores `Rust tests and clippy (ubuntu)`'s entry, which is a strict
//     superset of it. Both jobs compiled the identical `-C metadata` hashes
//     (`mendimaru_lib-eaf0963741e7e659`, `cli_contract-d17da48c1a8ad371`,
//     `cli_e2e-d00026d4ea2f37cf`, `winboat_lifecycle_matrix-1793e6e1a0d5b69d`)
//     in run 35818611312, so one entry serves both with no hit lost.
export const DEFAULT_RETIRED_FAMILIES = ["v0-rust-test-ubuntu-"];

export const DEFAULT_REF = "refs/heads/main";

const HEX_SEGMENT = /^[0-9a-f]{8,}$/i;
const RUN_ID_SEGMENT = /^[0-9]{6,}$/;
const PULL_REQUEST_REF = /^refs\/pull\/(\d+)\/(merge|head)$/;

// A cache key is `<stable name>-<volatile suffix...>`: a content hash, a run
// id, or both. Stripping the volatile tail groups the generations of one cache
// without hard-coding every key this repository happens to use today, so a
// cache added later is still bounded instead of silently unmanaged.
export function cacheFamily(key) {
  if (typeof key !== "string" || key.trim() === "") {
    throw new Error("a cache entry must have a non-empty key");
  }
  const segments = key.split("-");
  let end = segments.length;
  while (end > 1 && isVolatileSegment(segments[end - 1])) end -= 1;
  return segments.slice(0, end).join("-");
}

export function retentionFor(
  family,
  retention = DEFAULT_RETENTION,
  fallback = DEFAULT_KEEP,
) {
  for (const rule of retention) {
    if (family === rule.prefix || family.startsWith(rule.prefix)) {
      return rule.keep;
    }
  }
  return fallback;
}

// Retirement is matched the same way retention is, so a retired family covers
// every platform and architecture variant of one consolidated key at once.
export function isRetiredFamily(family, retired = DEFAULT_RETIRED_FAMILIES) {
  return retired.some(
    (prefix) => family === prefix || family.startsWith(prefix),
  );
}

// A cache is readable by the run whose ref saved it and, for a pull request,
// by the default branch's entries as a fallback - never the other way round.
// So a pull-request entry can only ever be restored by that pull request, and
// once the pull request is closed nothing can read it again.
export function pullRequestNumber(ref) {
  const match = PULL_REQUEST_REF.exec(String(ref ?? ""));
  return match ? Number(match[1]) : null;
}

export function planCacheDeletions(entries, options = {}) {
  // Fail closed. An unreadable listing must never be read as "there is nothing
  // to prune"; the caller sees an error instead of a silent no-op that leaves
  // the repository over the cap for another week.
  if (!Array.isArray(entries)) {
    throw new Error("expected an array of cache entries");
  }
  const {
    budgetBytes = DEFAULT_BUDGET_BYTES,
    capBytes = DEFAULT_CAP_BYTES,
    idleMinutes = DEFAULT_IDLE_MINUTES,
    retention = DEFAULT_RETENTION,
    keep = DEFAULT_KEEP,
    retiredFamilies = DEFAULT_RETIRED_FAMILIES,
    // How many bytes under the cap to keep free for one in-flight save.
    // `null` measures it from the listing, which is the only honest default:
    // the biggest save this repository can make is the biggest entry it
    // already holds. `0` is the pre-#209 behaviour and exists so a test can
    // isolate the budget rail from this reservation.
    reserveBytes = null,
    defaultRef = DEFAULT_REF,
    // `null` means the caller could not determine which pull requests are
    // open. That must not read as "every pull request is closed", so the dead
    // scope pass is skipped rather than deleting every pull-request entry.
    openPullRequests = null,
    now = Date.now(),
  } = options;
  if (!Number.isFinite(budgetBytes) || budgetBytes <= 0) {
    throw new Error("budgetBytes must be a positive number");
  }
  if (!Number.isFinite(capBytes) || capBytes <= 0) {
    throw new Error("capBytes must be a positive number");
  }
  // A budget above the cap would silently disable the error condition, which
  // is the one signal in this plan that something is actually broken.
  if (budgetBytes > capBytes) {
    throw new Error("budgetBytes must not exceed capBytes");
  }
  if (!Number.isFinite(idleMinutes) || idleMinutes < 0) {
    throw new Error("idleMinutes must be zero or a positive number");
  }
  if (
    reserveBytes !== null &&
    (!Number.isFinite(reserveBytes) || reserveBytes < 0)
  ) {
    throw new Error("reserveBytes must be null or zero or a positive number");
  }
  if (!Number.isFinite(now)) {
    throw new Error("now must be a finite timestamp");
  }
  if (typeof defaultRef !== "string" || defaultRef.trim() === "") {
    throw new Error("defaultRef must be a non-empty ref");
  }
  if (openPullRequests !== null && !Array.isArray(openPullRequests)) {
    throw new Error("openPullRequests must be an array or null");
  }
  if (!Array.isArray(retention)) {
    throw new Error("retention must be an array");
  }
  if (!Array.isArray(retiredFamilies)) {
    throw new Error("retiredFamilies must be an array");
  }
  for (const [index, prefix] of retiredFamilies.entries()) {
    if (typeof prefix !== "string" || prefix.trim() === "") {
      throw new Error(`retired family ${index} is not a usable prefix`);
    }
  }
  if (!Number.isSafeInteger(keep) || keep < 1) {
    throw new Error("keep must be a positive integer");
  }
  for (const [index, rule] of retention.entries()) {
    if (
      !rule ||
      typeof rule !== "object" ||
      typeof rule.prefix !== "string" ||
      rule.prefix.trim() === "" ||
      !Number.isSafeInteger(rule.keep) ||
      rule.keep < 1
    ) {
      throw new Error(`retention rule ${index} is invalid`);
    }
  }
  const openNumbers =
    openPullRequests === null
      ? null
      : new Set(openPullRequests.map(openPullRequestNumber));

  const normalised = entries.map(normaliseEntry);
  const ids = new Set();
  for (const entry of normalised) {
    if (ids.has(entry.id)) {
      throw new Error(`duplicate cache id ${entry.id}`);
    }
    ids.add(entry.id);
  }
  const totalBytes = sum(normalised);
  const idleCutoff = now - idleMinutes * 60_000;

  // #209's actual invariant. Staying under the cap is not enough; the cap has
  // to be able to absorb one more save of the biggest thing this repository
  // saves. On the 2026-09-23 listing it could not: 9.06 GiB remaining left
  // 0.94 GiB of headroom against a 1.12 GiB save, so a badly timed save still
  // crossed the cap and GitHub still evicted, which is #182 all over again.
  //
  // The size of that save is measurable rather than a guess: it is the
  // largest entry in the listing. Taking the maximum over *every* entry and
  // not only over the survivors is deliberate - a generation this pass is
  // about to delete is one its job will save again, so sizing the reservation
  // by it is the conservative reading.
  const largestSaveBytes = normalised.reduce(
    (largest, entry) => Math.max(largest, entry.sizeBytes),
    0,
  );
  const effectiveReserveBytes =
    reserveBytes === null ? largestSaveBytes : reserveBytes;
  // The effective target is the stricter of the reservation and the standing
  // policy budget. It can only ever tighten, never loosen, so adding it
  // cannot turn an existing warning green.
  const reserveBudgetBytes = Math.max(0, capBytes - effectiveReserveBytes);
  const policyBudgetBytes = budgetBytes;
  const effectiveBudgetBytes = Math.min(policyBudgetBytes, reserveBudgetBytes);

  const deletions = [];
  const deferred = [];
  const live = [];
  // Dead pull-request scopes first. They are the cheapest bytes in the
  // repository to reclaim - nothing can restore them - and here they are not
  // marginal: every pull request saves its own copy of the baseline
  // dependency caches, 694 MB on Linux and 628 MB on Windows.
  for (const entry of normalised) {
    const pullRequest = pullRequestNumber(entry.ref);
    const dead =
      openNumbers !== null &&
      pullRequest !== null &&
      !openNumbers.has(pullRequest);
    if (!dead) {
      live.push(entry);
    } else if (entry.lastAccessedAt > idleCutoff) {
      deferred.push({ ...entry, reason: "recently-used" });
      live.push(entry);
    } else {
      deletions.push({ ...entry, reason: "closed-pull-request" });
    }
  }

  // Retired families next, for the same reason and with the same guard: they
  // are bytes nothing can restore, so reclaiming them costs no cache hit, but
  // a run that restored one seconds ago is still reading it. Running this
  // before the supersession pass keeps a retired family's last generation out
  // of the retained set, which is the set the budget pass refuses to touch.
  const kept = [];
  for (const entry of live) {
    if (!isRetiredFamily(entry.family, retiredFamilies)) {
      kept.push(entry);
    } else if (entry.lastAccessedAt > idleCutoff) {
      deferred.push({ ...entry, reason: "recently-used" });
      kept.push(entry);
    } else {
      deletions.push({ ...entry, reason: "retired-family" });
    }
  }

  // Supersession is scoped per ref, because a pull-request entry never
  // supersedes the default branch's: the two are restored by different runs.
  // Grouping them together would delete the shared copy in favour of one pull
  // request's private copy.
  const families = new Map();
  for (const entry of kept) {
    const scope = `${entry.ref}\u0000${entry.family}`;
    const bucket = families.get(scope);
    if (bucket) bucket.push(entry);
    else families.set(scope, [entry]);
  }

  const survivors = [];
  for (const [scope, bucket] of [...families].sort(byScopeName)) {
    const family = scope.slice(scope.indexOf("\u0000") + 1);
    const keepCount = Math.max(1, retentionFor(family, retention, keep));
    [...bucket].sort(newestFirst).forEach((entry, index) => {
      const shared = entry.ref === defaultRef;
      if (index < keepCount) {
        survivors.push({ ...entry, retained: true, shared });
        return;
      }
      if (entry.lastAccessedAt > idleCutoff) {
        // Superseded, but a run may still be reading it. Deferring costs one
        // prune cycle; deleting costs a red job in somebody else's run.
        deferred.push({ ...entry, reason: "recently-used" });
        survivors.push({ ...entry, retained: false, shared });
        return;
      }
      deletions.push({ ...entry, reason: "superseded" });
    });
  }

  // Last pass: the surviving generations can still exceed the budget - a
  // toolchain roll leaves two live generations of every dependency cache.
  // Evict least-recently-used first, the same order GitHub would use, but in
  // two tiers: superseded-yet-deferred-free entries before a pull request's
  // private copy, and never the default branch's retained entries. Losing a
  // shared entry costs every branch a cold rebuild, which is the exact failure
  // this plan exists to prevent.
  let remainingBytes = totalBytes - sum(deletions);
  const idle = survivors.filter((entry) => entry.lastAccessedAt <= idleCutoff);
  const evictable = [
    ...idle.filter((entry) => !entry.retained).sort(leastRecentlyUsedFirst),
    ...idle
      .filter((entry) => entry.retained && !entry.shared)
      .sort(leastRecentlyUsedFirst),
  ];
  for (const entry of evictable) {
    if (remainingBytes <= effectiveBudgetBytes) break;
    deletions.push({ ...entry, reason: "budget" });
    remainingBytes -= entry.sizeBytes;
  }

  const sharedRetainedBytes = sum(
    survivors.filter((entry) => entry.retained && entry.shared),
  );
  // What the cap can still absorb once this plan has been applied, against
  // what one more save of the largest family costs. This is the pair #209 is
  // about: a repository *under* the cap still loses a dependency cache to
  // eviction when the first number is smaller than the second.
  const saveHeadroomBytes = capBytes - remainingBytes;
  const structuralSaveHeadroomBytes = capBytes - sharedRetainedBytes;
  return {
    totalBytes,
    // The target everything below is judged against: the stricter of the
    // standing policy budget and `cap - reservation`. Reported as
    // `budgetBytes` so the workflow and the report keep speaking about one
    // number, with the inputs alongside it for the log.
    budgetBytes: effectiveBudgetBytes,
    policyBudgetBytes,
    reserveBudgetBytes,
    reserveBytes: effectiveReserveBytes,
    // Which of the two constraints is actually binding, so the report can say
    // why the target is what it is instead of quoting a bare number.
    budgetBinding:
      reserveBudgetBytes < policyBudgetBytes ? "reserve" : "policy",
    capBytes,
    largestSaveBytes,
    saveHeadroomBytes,
    structuralSaveHeadroomBytes,
    // The #209 acceptance criterion, as a computed predicate rather than a
    // number somebody has to re-derive: can the cap absorb one more save of
    // the largest family on top of what this plan leaves behind?
    fitsReserve: saveHeadroomBytes >= effectiveReserveBytes,
    structurallyFitsReserve:
      structuralSaveHeadroomBytes >= effectiveReserveBytes,
    // A single entry at or above the cap can never be saved safely, whatever
    // the rest of the listing does. Pruning cannot reach it, so it is an
    // error about the cached paths, not about this plan.
    reserveExceedsCap: effectiveReserveBytes >= capBytes,
    freedBytes: sum(deletions),
    remainingBytes,
    sharedRetainedBytes,
    overBudget: remainingBytes > effectiveBudgetBytes,
    // Distinguishes "the prune still has work to do" from "retention itself
    // does not fit under the cap". The second is a policy problem no amount of
    // pruning fixes, and it is the one worth reporting loudly.
    structurallyOverBudget: sharedRetainedBytes > effectiveBudgetBytes,
    // The escalation of the same two questions against GitHub's real limit.
    // `overCap` means the prune failed at its actual job and the next run can
    // still lose a dependency cache to LRU eviction. `structurallyOverCap`
    // means the caches this repository insists on keeping do not fit at all,
    // so the cached paths themselves have to change.
    overCap: remainingBytes > capBytes,
    structurallyOverCap: sharedRetainedBytes > capBytes,
    deletions: deletions.sort(byIdAscending),
    deferred: deferred.sort(byIdAscending),
  };
}

export function renderPlan(plan) {
  const count = plan.deletions.length;
  const lines = [
    `Actions cache budget: ${gib(plan.totalBytes)} used against a ${gib(plan.budgetBytes)} budget; ${count} entr${count === 1 ? "y" : "ies"} to delete.`,
  ];
  for (const entry of plan.deletions) {
    lines.push(
      `  delete ${entry.key} [${entry.ref}] (${gib(entry.sizeBytes)}, ${entry.reason})`,
    );
  }
  for (const entry of plan.deferred) {
    lines.push(
      `  defer  ${entry.key} [${entry.ref}] (${gib(entry.sizeBytes)}, dead or superseded but restored inside the idle window)`,
    );
  }
  lines.push(`After pruning: ${gib(plan.remainingBytes)}.`);
  // The two numbers #209 is about, always printed, because the failure it
  // describes is invisible in the total alone: a repository can be under the
  // cap and still lose a cache to the next save.
  lines.push(
    `Largest single save ${gib(plan.largestSaveBytes)} against ${gib(plan.saveHeadroomBytes)} of headroom under the ${gib(plan.capBytes)} cap` +
      (plan.budgetBinding === "reserve"
        ? `; the ${gib(plan.reserveBytes)} reservation sets the target, not the ${gib(plan.policyBudgetBytes)} policy budget.`
        : `; the ${gib(plan.policyBudgetBytes)} policy budget sets the target.`),
  );
  // Severity follows the cap, not the budget. Being over the budget but under
  // the cap is the expected steady state of this repository today and must not
  // read like a failure, or the one line that does mean failure gets ignored.
  if (plan.structurallyOverCap) {
    lines.push(
      `ERROR: the default branch's retained generations alone are ${gib(plan.sharedRetainedBytes)}, over GitHub's ${gib(plan.capBytes)} cap. Pruning cannot fix that; the retention policy or the cached paths have to change.`,
    );
  } else if (plan.overCap) {
    lines.push(
      `ERROR: still ${gib(plan.remainingBytes - plan.capBytes)} over GitHub's ${gib(plan.capBytes)} cap after pruning; the next run can still lose a dependency cache to eviction.`,
    );
  } else if (plan.reserveExceedsCap) {
    lines.push(
      `ERROR: one save alone needs ${gib(plan.reserveBytes)}, at or over GitHub's ${gib(plan.capBytes)} cap. No prune can make that save safe; the cached paths have to change.`,
    );
  } else if (plan.structurallyOverBudget) {
    lines.push(
      `WARNING: the default branch's retained generations alone are ${gib(plan.sharedRetainedBytes)}, over the ${gib(plan.budgetBytes)} headroom target but within GitHub's ${gib(plan.capBytes)} cap. That leaves ${gib(plan.structuralSaveHeadroomBytes)} for a ${gib(plan.largestSaveBytes)} save. Pruning cannot reclaim these; only changing what is cached can.`,
    );
  } else if (plan.overBudget) {
    lines.push(
      `WARNING: still ${gib(plan.remainingBytes - plan.budgetBytes)} over the headroom target after pruning, but within GitHub's ${gib(plan.capBytes)} cap; the remainder is retained or inside the idle window.`,
    );
  }
  return lines.join("\n");
}

function isVolatileSegment(segment) {
  return HEX_SEGMENT.test(segment) || RUN_ID_SEGMENT.test(segment);
}

function normaliseEntry(entry, index) {
  if (!entry || typeof entry !== "object") {
    throw new Error(`cache entry ${index} is not an object`);
  }
  // `gh cache list --json` and the REST API spell the same fields differently.
  // Accepting both keeps the workflow free to use either without the plan
  // silently reading every size as zero.
  const id = Number(entry.id);
  if (!Number.isSafeInteger(id) || id <= 0) {
    throw new Error(`cache entry ${index} has no usable id`);
  }
  const key = String(entry.key ?? "");
  const sizeBytes = Number(entry.sizeInBytes ?? entry.size_in_bytes);
  if (!Number.isFinite(sizeBytes) || sizeBytes < 0) {
    throw new Error(`cache entry ${key || index} has no usable size`);
  }
  const createdAt = parseTime(entry.createdAt ?? entry.created_at, key, index);
  const lastAccessedAt = parseLastAccessedTime(
    entry.lastAccessedAt ?? entry.last_accessed_at,
    key,
    index,
  );
  const ref = String(entry.ref ?? "");
  if (ref.trim() === "") {
    throw new Error(`cache entry ${key || index} has no usable ref`);
  }
  return {
    id,
    key,
    family: cacheFamily(key),
    ref,
    sizeBytes,
    createdAt,
    // A never-restored entry is as old as its creation, which is exactly how
    // it should rank for eviction.
    lastAccessedAt: lastAccessedAt ?? createdAt,
  };
}

function parseTime(value, key, index) {
  const parsed = parseOptionalTime(value);
  if (parsed === null) {
    throw new Error(`cache entry ${key || index} has no usable creation time`);
  }
  return parsed;
}

function parseOptionalTime(value) {
  if (value === undefined || value === null || value === "") return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}

function parseLastAccessedTime(value, key, index) {
  if (value === undefined || value === null || value === "") return null;
  const parsed = parseOptionalTime(value);
  if (parsed === null) {
    throw new Error(
      `cache entry ${key || index} has no usable last-accessed time`,
    );
  }
  return parsed;
}

function openPullRequestNumber(value, index) {
  const number = Number(value?.number ?? value);
  if (!Number.isSafeInteger(number) || number <= 0) {
    throw new Error(`open pull request ${index} has no usable number`);
  }
  return number;
}

function sum(entries) {
  return entries.reduce((total, entry) => total + entry.sizeBytes, 0);
}

function newestFirst(left, right) {
  return (
    right.createdAt - left.createdAt ||
    right.lastAccessedAt - left.lastAccessedAt ||
    right.id - left.id
  );
}

function leastRecentlyUsedFirst(left, right) {
  return (
    left.lastAccessedAt - right.lastAccessedAt ||
    right.sizeBytes - left.sizeBytes ||
    left.id - right.id
  );
}

function byScopeName(left, right) {
  return left[0] < right[0] ? -1 : left[0] > right[0] ? 1 : 0;
}

function byIdAscending(left, right) {
  return left.id - right.id;
}

function gib(bytes) {
  return `${(bytes / GIB).toFixed(2)} GiB`;
}

const invokedDirectly =
  process.argv[1] && process.argv[1].endsWith("cache-budget.mjs");
if (invokedDirectly) {
  const options = {};
  let planFile = "";
  let openPullRequestsFile = "";
  for (const argument of process.argv.slice(2)) {
    const separator = argument.indexOf("=");
    const name = separator === -1 ? argument : argument.slice(0, separator);
    const value = separator === -1 ? "" : argument.slice(separator + 1);
    if (name === "--budget-gib") options.budgetBytes = Number(value) * GIB;
    else if (name === "--budget-bytes") options.budgetBytes = Number(value);
    else if (name === "--cap-gib") options.capBytes = Number(value) * GIB;
    else if (name === "--cap-bytes") options.capBytes = Number(value);
    // The headroom kept free for one in-flight save. Omitted, it is measured
    // from the listing; `--reserve-gib=0` asks what the plan looks like with
    // no reservation at all, which is what this job did before #209.
    else if (name === "--reserve-gib")
      options.reserveBytes = Number(value) * GIB;
    else if (name === "--reserve-bytes") options.reserveBytes = Number(value);
    else if (name === "--idle-minutes") options.idleMinutes = Number(value);
    else if (name === "--default-ref") options.defaultRef = value;
    else if (name === "--open-pull-requests") openPullRequestsFile = value;
    else if (name === "--plan-file") planFile = value;
    else throw new Error(`unknown argument: ${argument}`);
  }
  const { readFileSync, writeFileSync } = await import("node:fs");
  if (openPullRequestsFile) {
    options.openPullRequests = JSON.parse(
      readFileSync(openPullRequestsFile, "utf8"),
    );
  }
  const raw = await readStdin();
  const plan = planCacheDeletions(
    JSON.parse(raw.trim() === "" ? "[]" : raw),
    options,
  );
  // The report goes to the log and the machine-readable plan to a file, so a
  // human reading the job sees why an entry was deleted without parsing JSON.
  process.stderr.write(`${renderPlan(plan)}\n`);
  const serialised = JSON.stringify(plan);
  if (planFile) writeFileSync(planFile, serialised);
  process.stdout.write(serialised);
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
