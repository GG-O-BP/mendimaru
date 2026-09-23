import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  DEFAULT_BUDGET_BYTES,
  DEFAULT_CAP_BYTES,
  GIB,
  cacheFamily,
  planCacheDeletions,
  pullRequestNumber,
  renderPlan,
  retentionFor,
} from "./cache-budget.mjs";

// The real listing on 2026-09-23T02:20Z, the run that measured #182's cause
// (2). 10.31 GiB across 26 entries against GitHub's 10 GiB cap, with pull
// request 198 already merged and 199 still open.
// [id, key, ref, sizeBytes, createdAt, lastAccessedAt]
const OBSERVED = [
  [
    7445146100,
    "node-cache-Windows-x64-npm-f1eb64c705c69bae23808d04d8bf36ba4f10fc047746a0cc226c846cf3bd1f4f",
    "refs/heads/main",
    44279883,
    "2026-09-08T07:56:46Z",
    "2026-09-23T02:10:08Z",
  ],
  [
    7714433869,
    "v0-rust-test-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    1075246156,
    "2026-09-15T12:01:44Z",
    "2026-09-23T02:08:12Z",
  ],
  [
    7714772507,
    "v0-rust-windows-native-e2e-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    745484792,
    "2026-09-15T12:10:56Z",
    "2026-09-23T02:08:25Z",
  ],
  [
    7715297931,
    "v0-rust-windows-bundle-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    612552955,
    "2026-09-15T12:24:56Z",
    "2026-09-23T02:08:23Z",
  ],
  [
    7715472568,
    "node-cache-Linux-x64-npm-e48b10e82a1d25ce50fec4106003fe6d049933989d4de7c565177321358f7f2e",
    "refs/heads/main",
    54592811,
    "2026-09-15T12:29:28Z",
    "2026-09-23T02:08:29Z",
  ],
  [
    7925367314,
    "tauri-driver-2.0.6-x86_64-unknown-linux-gnu",
    "refs/heads/main",
    824488,
    "2026-09-21T09:28:49Z",
    "2026-09-23T02:08:53Z",
  ],
  [
    7925375002,
    "v0-rust-linux-webkit-e2e-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    913393176,
    "2026-09-21T09:29:04Z",
    "2026-09-23T02:08:31Z",
  ],
  [
    7925426914,
    "playwright-chromium-Linux-e48b10e82a1d25ce50fec4106003fe6d049933989d4de7c565177321358f7f2e",
    "refs/heads/main",
    281922771,
    "2026-09-21T09:30:20Z",
    "2026-09-23T02:08:53Z",
  ],
  [
    7925436542,
    "v0-rust-test-ubuntu-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    868309554,
    "2026-09-21T09:30:36Z",
    "2026-09-23T02:08:08Z",
  ],
  [
    7925484967,
    "v0-rust-rust-ubuntu-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    1198484198,
    "2026-09-21T09:31:52Z",
    "2026-09-23T02:08:19Z",
  ],
  [
    7925535131,
    "v0-rust-release-performance-linux-candidate-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    694340957,
    "2026-09-21T09:33:07Z",
    "2026-09-23T02:01:44Z",
  ],
  [
    8002775953,
    "v0-rust-release-performance-windows-candidate-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    627997049,
    "2026-09-23T01:26:05Z",
    "2026-09-23T02:01:47Z",
  ],
  [
    8002777394,
    "webview-binary-windows-fdaaf369606dd2a903b3aef69e3d296b",
    "refs/heads/main",
    11071230,
    "2026-09-23T01:26:06Z",
    "2026-09-23T01:51:50Z",
  ],
  [
    8002805889,
    "v0-rust-release-performance-windows-bundles-candidate-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    612367859,
    "2026-09-23T01:27:22Z",
    "2026-09-23T01:52:11Z",
  ],
  [
    8002807183,
    "bundle-installers-1b90b2d0aec94443cd106ca94d244e38",
    "refs/heads/main",
    25340563,
    "2026-09-23T01:27:23Z",
    "2026-09-23T01:51:49Z",
  ],
  [
    8003414842,
    "webview-binary-linux-ef6ab146bedd70a957b9f4d6b93dc7cc",
    "refs/heads/main",
    92614772,
    "2026-09-23T01:55:42Z",
    "2026-09-23T02:07:41Z",
  ],
  [
    8003432059,
    "webview-binary-windows-32ad35d1df9bbcdb3ae8f9d21197ac0f",
    "refs/heads/main",
    11071219,
    "2026-09-23T01:56:33Z",
    "2026-09-23T02:07:56Z",
  ],
  [
    8003437501,
    "bundle-installers-68f4d87ef47b7d29bc9a1da1193e88e0",
    "refs/heads/main",
    25350938,
    "2026-09-23T01:56:51Z",
    "2026-09-23T02:07:54Z",
  ],
  [
    8003452366,
    "aur-sccache-v1-26e874bb89eaff99c879f1140c36cd9438b666eb41f30d847ece263747a19f0a-35808020907",
    "refs/heads/main",
    950520135,
    "2026-09-23T01:57:36Z",
    "2026-09-23T02:08:14Z",
  ],
  [
    8003454638,
    "webview-binary-linux-68af86ea9e0671d72bd40d458f431d92",
    "refs/pull/198/merge",
    92607785,
    "2026-09-23T01:57:43Z",
    "2026-09-23T01:57:43Z",
  ],
  [
    8003477963,
    "webview-binary-windows-32ad35d1df9bbcdb3ae8f9d21197ac0f",
    "refs/pull/198/merge",
    11071232,
    "2026-09-23T01:58:54Z",
    "2026-09-23T01:58:54Z",
  ],
  [
    8003511373,
    "v0-rust-release-performance-linux-baseline-Linux-x64-6ff13d87-aea8b408",
    "refs/pull/198/merge",
    694421025,
    "2026-09-23T02:00:28Z",
    "2026-09-23T02:00:28Z",
  ],
  [
    8003575878,
    "v0-rust-release-performance-windows-baseline-Windows_NT-x64-2113753f-aea8b408",
    "refs/pull/198/merge",
    627995589,
    "2026-09-23T02:03:31Z",
    "2026-09-23T02:03:31Z",
  ],
  [
    8003615160,
    "webview-binary-linux-8ae513f7f98866728143f6d512eda5fa",
    "refs/pull/199/merge",
    92624291,
    "2026-09-23T02:05:20Z",
    "2026-09-23T02:06:37Z",
  ],
  [
    8003627366,
    "webview-binary-windows-3330f0049861a3a6c5b2146ed561fa79",
    "refs/pull/199/merge",
    11071220,
    "2026-09-23T02:05:55Z",
    "2026-09-23T02:07:00Z",
  ],
  [
    8003835947,
    "v0-rust-release-performance-linux-baseline-Linux-x64-6ff13d87-aea8b408",
    "refs/pull/199/merge",
    694450736,
    "2026-09-23T02:15:09Z",
    "2026-09-23T02:15:09Z",
  ],
].map(([id, key, ref, sizeInBytes, createdAt, lastAccessedAt]) => ({
  id,
  key,
  ref,
  sizeInBytes,
  createdAt,
  lastAccessedAt,
}));

const OBSERVED_AT = Date.parse("2026-09-23T02:20:00Z");

// The same repository ten hours later, after pull request 199 merged: 11.90
// GiB across 28 entries, well over the 10 GiB cap. This listing is why the cap
// and the headroom target had to become two different numbers. Pruning it
// reclaims everything that is safely reclaimable - three superseded
// `aur-sccache-v1` generations and the merged pull request 199 scope - and
// still lands at 9.06 GiB, because the ten `v0-rust-*` dependency families are
// 7.49 GiB and already hold exactly one generation each. Shrinking them means
// giving up a cache hit, which is the regression #182 exists to prevent.
const SATURATED = [
  [
    7445146100,
    "node-cache-Windows-x64-npm-f1eb64c705c69bae23808d04d8bf36ba4f10fc047746a0cc226c846cf3bd1f4f",
    "refs/heads/main",
    44279883,
    "2026-09-08T07:56:46.789493Z",
    "2026-09-23T03:46:27.324307Z",
  ],
  [
    7714433869,
    "v0-rust-test-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    1075246156,
    "2026-09-15T12:01:44.457232Z",
    "2026-09-23T03:33:54.256251Z",
  ],
  [
    7714772507,
    "v0-rust-windows-native-e2e-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    745484792,
    "2026-09-15T12:10:56.365834Z",
    "2026-09-23T03:33:34.226766Z",
  ],
  [
    7715297931,
    "v0-rust-windows-bundle-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    612552955,
    "2026-09-15T12:24:56.059268Z",
    "2026-09-23T03:33:41.870854Z",
  ],
  [
    7715472568,
    "node-cache-Linux-x64-npm-e48b10e82a1d25ce50fec4106003fe6d049933989d4de7c565177321358f7f2e",
    "refs/heads/main",
    54592811,
    "2026-09-15T12:29:28.904753Z",
    "2026-09-23T03:52:38.12579Z",
  ],
  [
    7925367314,
    "tauri-driver-2.0.6-x86_64-unknown-linux-gnu",
    "refs/heads/main",
    824488,
    "2026-09-21T09:28:49.938282Z",
    "2026-09-23T03:46:48.922215Z",
  ],
  [
    7925375002,
    "v0-rust-linux-webkit-e2e-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    913393176,
    "2026-09-21T09:29:04.23358Z",
    "2026-09-23T03:33:40.163988Z",
  ],
  [
    7925426914,
    "playwright-chromium-Linux-e48b10e82a1d25ce50fec4106003fe6d049933989d4de7c565177321358f7f2e",
    "refs/heads/main",
    281922771,
    "2026-09-21T09:30:20.191372Z",
    "2026-09-23T03:35:26.809304Z",
  ],
  [
    7925436542,
    "v0-rust-test-ubuntu-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    868309554,
    "2026-09-21T09:30:36.733888Z",
    "2026-09-23T03:35:03.101728Z",
  ],
  [
    7925484967,
    "v0-rust-rust-ubuntu-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    1198484198,
    "2026-09-21T09:31:52.885416Z",
    "2026-09-23T03:33:40.868338Z",
  ],
  [
    8003414842,
    "webview-binary-linux-ef6ab146bedd70a957b9f4d6b93dc7cc",
    "refs/heads/main",
    92614772,
    "2026-09-23T01:55:42.627862Z",
    "2026-09-23T03:20:09.29338Z",
  ],
  [
    8003432059,
    "webview-binary-windows-32ad35d1df9bbcdb3ae8f9d21197ac0f",
    "refs/heads/main",
    11071219,
    "2026-09-23T01:56:33.860047Z",
    "2026-09-23T03:33:14.200143Z",
  ],
  [
    8003437501,
    "bundle-installers-68f4d87ef47b7d29bc9a1da1193e88e0",
    "refs/heads/main",
    25350938,
    "2026-09-23T01:56:51.535918Z",
    "2026-09-23T03:33:15.057699Z",
  ],
  [
    8003615160,
    "webview-binary-linux-8ae513f7f98866728143f6d512eda5fa",
    "refs/pull/199/merge",
    92624291,
    "2026-09-23T02:05:20.749955Z",
    "2026-09-23T03:20:23.530393Z",
  ],
  [
    8003627366,
    "webview-binary-windows-3330f0049861a3a6c5b2146ed561fa79",
    "refs/pull/199/merge",
    11071220,
    "2026-09-23T02:05:55.316177Z",
    "2026-09-23T03:20:48.283797Z",
  ],
  [
    8003837173,
    "webview-binary-linux-68af86ea9e0671d72bd40d458f431d92",
    "refs/pull/199/merge",
    92609306,
    "2026-09-23T02:15:10.715803Z",
    "2026-09-23T03:20:25.288028Z",
  ],
  [
    8003848150,
    "aur-sccache-v1-26e874bb89eaff99c879f1140c36cd9438b666eb41f30d847ece263747a19f0a-35809104497",
    "refs/heads/main",
    950523764,
    "2026-09-23T02:15:42.125945Z",
    "2026-09-23T02:45:38.09424Z",
  ],
  [
    8003866687,
    "v0-rust-release-performance-linux-baseline-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    694422570,
    "2026-09-23T02:16:32.617302Z",
    "2026-09-23T02:16:32.617302Z",
  ],
  [
    8003867920,
    "webview-binary-linux-68af86ea9e0671d72bd40d458f431d92",
    "refs/heads/main",
    92609600,
    "2026-09-23T02:16:35.580347Z",
    "2026-09-23T03:33:01.503373Z",
  ],
  [
    8004542603,
    "aur-sccache-v1-26e874bb89eaff99c879f1140c36cd9438b666eb41f30d847ece263747a19f0a-35811293820",
    "refs/heads/main",
    950527563,
    "2026-09-23T02:48:24.101117Z",
    "2026-09-23T03:22:56.962223Z",
  ],
  [
    8005435155,
    "aur-sccache-v1-26e874bb89eaff99c879f1140c36cd9438b666eb41f30d847ece263747a19f0a-35813898003",
    "refs/heads/main",
    950643176,
    "2026-09-23T03:27:56.691683Z",
    "2026-09-23T03:33:12.214502Z",
  ],
  [
    8005737901,
    "aur-sccache-v1-26e874bb89eaff99c879f1140c36cd9438b666eb41f30d847ece263747a19f0a-35814757141",
    "refs/heads/main",
    950524153,
    "2026-09-23T03:41:14.981097Z",
    "2026-09-23T03:41:14.981097Z",
  ],
  [
    8005753116,
    "v0-rust-release-performance-linux-candidate-Linux-x64-6ff13d87-aea8b408",
    "refs/heads/main",
    694333530,
    "2026-09-23T03:41:36.435984Z",
    "2026-09-23T03:41:36.435984Z",
  ],
  [
    8005754317,
    "webview-binary-linux-8ae513f7f98866728143f6d512eda5fa",
    "refs/heads/main",
    92602285,
    "2026-09-23T03:41:38.825144Z",
    "2026-09-23T03:41:38.825144Z",
  ],
  [
    8005808409,
    "v0-rust-release-performance-windows-bundles-candidate-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    612118785,
    "2026-09-23T03:44:12.397642Z",
    "2026-09-23T03:44:12.397642Z",
  ],
  [
    8005809894,
    "bundle-installers-d49cfbe60f16b36e551d084b0ba4dd4c",
    "refs/heads/main",
    25351063,
    "2026-09-23T03:44:14.641448Z",
    "2026-09-23T03:44:14.641448Z",
  ],
  [
    8005844356,
    "v0-rust-release-performance-windows-candidate-Windows_NT-x64-2113753f-aea8b408",
    "refs/heads/main",
    628138294,
    "2026-09-23T03:45:52.319508Z",
    "2026-09-23T03:45:52.319508Z",
  ],
  [
    8005845343,
    "webview-binary-windows-3330f0049861a3a6c5b2146ed561fa79",
    "refs/heads/main",
    11071224,
    "2026-09-23T03:45:53.629291Z",
    "2026-09-23T03:45:53.629291Z",
  ],
].map(([id, key, ref, sizeInBytes, createdAt, lastAccessedAt]) => ({
  id,
  key,
  ref,
  sizeInBytes,
  createdAt,
  lastAccessedAt,
}));

const SATURATED_AT = Date.parse("2026-09-23T04:10:00Z");

function entry(overrides) {
  return {
    id: 1,
    key: "v0-rust-test-ubuntu-Linux-x64-6ff13d87-aea8b408",
    ref: "refs/heads/main",
    sizeInBytes: 1_000,
    createdAt: "2026-09-23T00:00:00Z",
    lastAccessedAt: "2026-09-23T00:00:00Z",
    ...overrides,
  };
}

const NOW = Date.parse("2026-09-23T12:00:00Z");

test("generations of one cache collapse to one family", () => {
  assert.equal(
    cacheFamily("v0-rust-test-ubuntu-Linux-x64-6ff13d87-aea8b408"),
    "v0-rust-test-ubuntu-Linux-x64",
  );
  // Hash and run id both stripped, so every AUR compiler cache is one family.
  assert.equal(
    cacheFamily(
      "aur-sccache-v1-26e874bb89eaff99c879f1140c36cd9438b666eb41f30d847ece263747a19f0a-35808020907",
    ),
    "aur-sccache-v1",
  );
  assert.equal(
    cacheFamily("webview-binary-linux-8ae513f7f98866728143f6d512eda5fa"),
    "webview-binary-linux",
  );
  // Baseline and candidate stay separate: they are restored to different
  // paths, so one can never stand in for the other.
  assert.notEqual(
    cacheFamily(
      "v0-rust-release-performance-linux-baseline-Linux-x64-6ff13d87-aea8b408",
    ),
    cacheFamily(
      "v0-rust-release-performance-linux-candidate-Linux-x64-6ff13d87-aea8b408",
    ),
  );
});

test("a key that is all stable segments is its own family", () => {
  // `x64`, `gnu` and `Linux` are not hex; a version is not a run id. Stripping
  // any of them would merge unrelated caches into one family and delete live
  // entries.
  assert.equal(
    cacheFamily("tauri-driver-2.0.6-x86_64-unknown-linux-gnu"),
    "tauri-driver-2.0.6-x86_64-unknown-linux-gnu",
  );
  assert.equal(cacheFamily("solo"), "solo");
  // A key that is nothing but a hash still keeps one segment rather than
  // collapsing every such key into a single empty family.
  assert.equal(cacheFamily("deadbeefdeadbeef"), "deadbeefdeadbeef");
});

test("an unusable key is refused instead of silently grouped", () => {
  assert.throws(() => cacheFamily(""), /non-empty key/);
  assert.throws(() => cacheFamily("   "), /non-empty key/);
  assert.throws(() => cacheFamily(undefined), /non-empty key/);
});

test("retention follows how a family is restored, not how big it is", () => {
  assert.equal(retentionFor("v0-rust-test-ubuntu-Linux-x64"), 1);
  assert.equal(retentionFor("aur-sccache-v1"), 1);
  assert.equal(retentionFor("webview-binary-linux"), 4);
  assert.equal(retentionFor("bundle-installers"), 4);
  // Unclassified families keep two generations rather than being pruned to
  // one by a policy nobody checked against their restore semantics.
  assert.equal(retentionFor("playwright-chromium-Linux"), 2);
});

test("pull-request scopes are recognised and branch scopes are not", () => {
  assert.equal(pullRequestNumber("refs/pull/198/merge"), 198);
  assert.equal(pullRequestNumber("refs/pull/198/head"), 198);
  assert.equal(pullRequestNumber("refs/heads/main"), null);
  assert.equal(pullRequestNumber("refs/heads/refs/pull/1/merge"), null);
  assert.equal(pullRequestNumber(undefined), null);
});

test("an empty listing plans nothing and reports nothing over budget", () => {
  const plan = planCacheDeletions([], { now: NOW });
  assert.deepEqual(plan.deletions, []);
  assert.deepEqual(plan.deferred, []);
  assert.equal(plan.totalBytes, 0);
  assert.equal(plan.remainingBytes, 0);
  assert.equal(plan.overBudget, false);
  assert.equal(plan.structurallyOverBudget, false);
  assert.equal(plan.budgetBytes, DEFAULT_BUDGET_BYTES);
});

test("only the newest generation of a prefix-restored family survives", () => {
  const plan = planCacheDeletions(
    [
      entry({ id: 1, createdAt: "2026-09-20T00:00:00Z" }),
      entry({ id: 2, createdAt: "2026-09-21T00:00:00Z" }),
      entry({ id: 3, createdAt: "2026-09-22T00:00:00Z" }),
    ],
    { now: NOW },
  );
  assert.deepEqual(
    plan.deletions.map((deletion) => [deletion.id, deletion.reason]),
    [
      [1, "superseded"],
      [2, "superseded"],
    ],
  );
});

test("a fingerprint-keyed family keeps the history #178 restores from", () => {
  const binaries = [1, 2, 3, 4, 5].map((id) =>
    entry({
      id,
      key: `webview-binary-linux-${"0".repeat(31)}${id}`,
      sizeInBytes: 92_000_000,
      createdAt: `2026-09-${10 + id}T00:00:00Z`,
    }),
  );
  const plan = planCacheDeletions(binaries, { now: NOW });
  // Exactly the oldest one beyond the four retained generations.
  assert.deepEqual(
    plan.deletions.map((deletion) => deletion.id),
    [1],
  );
});

test("a superseded entry restored inside the idle window is deferred, not deleted", () => {
  const plan = planCacheDeletions(
    [
      entry({ id: 1, createdAt: "2026-09-20T00:00:00Z" }),
      entry({
        id: 2,
        createdAt: "2026-09-21T00:00:00Z",
        lastAccessedAt: "2026-09-23T11:58:00Z",
      }),
      entry({ id: 3, createdAt: "2026-09-22T00:00:00Z" }),
    ],
    { now: NOW },
  );
  assert.deepEqual(
    plan.deletions.map((deletion) => deletion.id),
    [1],
  );
  assert.deepEqual(
    plan.deferred.map((deferral) => [deferral.id, deferral.reason]),
    [[2, "recently-used"]],
  );
  // Deferred bytes are still counted as present; the plan never claims space
  // it did not reclaim.
  assert.equal(plan.remainingBytes, 2_000);
});

test("idleMinutes of zero deletes a just-restored superseded entry", () => {
  const plan = planCacheDeletions(
    [
      entry({
        id: 1,
        createdAt: "2026-09-20T00:00:00Z",
        lastAccessedAt: "2026-09-23T11:59:59Z",
      }),
      entry({ id: 2, createdAt: "2026-09-22T00:00:00Z" }),
    ],
    { now: NOW, idleMinutes: 0 },
  );
  assert.deepEqual(
    plan.deletions.map((deletion) => deletion.id),
    [1],
  );
  assert.deepEqual(plan.deferred, []);
});

test("a pull request's scope dies with the pull request", () => {
  const entries = [
    entry({ id: 1, ref: "refs/pull/198/merge", sizeInBytes: 694_000_000 }),
    entry({ id: 2, ref: "refs/pull/199/merge", sizeInBytes: 694_000_000 }),
    entry({ id: 3, ref: "refs/heads/main", sizeInBytes: 694_000_000 }),
  ];
  const plan = planCacheDeletions(entries, {
    now: NOW,
    openPullRequests: [{ number: 199 }],
  });
  assert.deepEqual(
    plan.deletions.map((deletion) => [deletion.id, deletion.reason]),
    [[1, "closed-pull-request"]],
  );
  // Plain numbers are accepted too, so the workflow can pass either shape.
  assert.deepEqual(
    planCacheDeletions(entries, { now: NOW, openPullRequests: [199] }).deletions
      .length,
    1,
  );
});

test("unknown pull-request state never reads as every pull request being closed", () => {
  const plan = planCacheDeletions(
    [entry({ id: 1, ref: "refs/pull/198/merge" })],
    { now: NOW },
  );
  assert.deepEqual(plan.deletions, []);
  assert.throws(
    () => planCacheDeletions([], { openPullRequests: "198" }),
    /openPullRequests must be an array or null/,
  );
  assert.throws(
    () => planCacheDeletions([], { openPullRequests: [{}] }),
    /open pull request 0 has no usable number/,
  );
  assert.throws(
    () => planCacheDeletions([], { openPullRequests: [0] }),
    /open pull request 0 has no usable number/,
  );
});

test("a pull request's copy never supersedes the shared copy it shadows", () => {
  // Same family, same key generation, two scopes. Only the default branch's
  // entry can be restored by other branches, so deleting it in favour of a
  // pull request's private copy is the one outcome that must not happen.
  const plan = planCacheDeletions(
    [
      entry({
        id: 1,
        ref: "refs/heads/main",
        createdAt: "2026-09-20T00:00:00Z",
        sizeInBytes: 694_000_000,
      }),
      entry({
        id: 2,
        ref: "refs/pull/199/merge",
        createdAt: "2026-09-22T00:00:00Z",
        sizeInBytes: 694_000_000,
      }),
    ],
    { now: NOW, openPullRequests: [199] },
  );
  assert.deepEqual(plan.deletions, []);
});

test("over budget, a pull request's private copy goes before the shared one", () => {
  const plan = planCacheDeletions(
    [
      entry({
        id: 1,
        ref: "refs/heads/main",
        sizeInBytes: 6 * GIB,
        lastAccessedAt: "2026-09-21T00:00:00Z",
      }),
      entry({
        id: 2,
        key: "v0-rust-rust-ubuntu-Linux-x64-6ff13d87-aea8b408",
        ref: "refs/pull/199/merge",
        sizeInBytes: 4 * GIB,
        lastAccessedAt: "2026-09-22T00:00:00Z",
      }),
    ],
    { now: NOW, openPullRequests: [199], budgetBytes: 9 * GIB },
  );
  assert.deepEqual(
    plan.deletions.map((deletion) => [deletion.id, deletion.reason]),
    [[2, "budget"]],
  );
  assert.equal(plan.remainingBytes, 6 * GIB);
  assert.equal(plan.overBudget, false);
});

test("budget eviction stops as soon as the listing fits", () => {
  const plan = planCacheDeletions(
    [1, 2, 3, 4].map((id) =>
      entry({
        id,
        key: `webview-binary-linux-${"0".repeat(31)}${id}`,
        ref: "refs/pull/199/merge",
        sizeInBytes: 3 * GIB,
        createdAt: `2026-09-1${id}T00:00:00Z`,
        lastAccessedAt: `2026-09-1${id}T00:00:00Z`,
      }),
    ),
    { now: NOW, openPullRequests: [199], budgetBytes: 9 * GIB },
  );
  // 12 GiB against a 9 GiB budget: one entry, the least recently used, and
  // then it stops rather than reclaiming everything it is allowed to.
  assert.deepEqual(
    plan.deletions.map((deletion) => [deletion.id, deletion.reason]),
    [[1, "budget"]],
  );
  assert.equal(plan.remainingBytes, 9 * GIB);
});

test("the shared retained set is never evicted, and saying so is the report", () => {
  const plan = planCacheDeletions(
    [
      entry({ id: 1, sizeInBytes: 6 * GIB }),
      entry({
        id: 2,
        key: "v0-rust-rust-ubuntu-Linux-x64-6ff13d87-aea8b408",
        sizeInBytes: 5 * GIB,
      }),
    ],
    { now: NOW, budgetBytes: 9 * GIB },
  );
  assert.deepEqual(plan.deletions, []);
  assert.equal(plan.overBudget, true);
  assert.equal(plan.structurallyOverBudget, true);
  assert.match(renderPlan(plan), /retention policy or the cached paths/);
});

test("still over budget after pruning is reported separately from a policy overrun", () => {
  const plan = planCacheDeletions(
    [
      entry({ id: 1, sizeInBytes: 5 * GIB }),
      entry({
        id: 2,
        key: "webview-binary-linux-0000000000000000000000000000000a",
        ref: "refs/pull/199/merge",
        sizeInBytes: 5 * GIB,
        lastAccessedAt: "2026-09-23T11:59:00Z",
      }),
    ],
    { now: NOW, budgetBytes: 9 * GIB, openPullRequests: [199] },
  );
  // The only evictable entry is inside the idle window, so this pass cannot
  // reach the budget and must not pretend otherwise.
  assert.deepEqual(plan.deletions, []);
  assert.equal(plan.overBudget, true);
  assert.equal(plan.structurallyOverBudget, false);
  // Over the headroom target but exactly at GitHub's cap, so this is a
  // warning and not the failure condition.
  assert.equal(plan.overCap, false);
  assert.equal(plan.structurallyOverCap, false);
  assert.match(
    renderPlan(plan),
    /WARNING: still 1\.00 GiB over the headroom target/,
  );
  assert.doesNotMatch(renderPlan(plan), /ERROR/);
});

test("both API spellings are read, and a never-restored entry ranks by creation", () => {
  const plan = planCacheDeletions(
    [
      {
        id: 1,
        key: "v0-rust-test-ubuntu-Linux-x64-6ff13d87-aea8b408",
        ref: "refs/heads/main",
        size_in_bytes: 4_000,
        created_at: "2026-09-20T00:00:00Z",
      },
      entry({ id: 2, createdAt: "2026-09-21T00:00:00Z", sizeInBytes: 1_000 }),
    ],
    { now: NOW },
  );
  assert.equal(plan.totalBytes, 5_000);
  assert.deepEqual(
    plan.deletions.map((deletion) => [deletion.id, deletion.sizeBytes]),
    [[1, 4_000]],
  );
});

test("an unreadable listing fails closed rather than planning nothing", () => {
  assert.throws(() => planCacheDeletions(null), /array of cache entries/);
  assert.throws(() => planCacheDeletions("[]"), /array of cache entries/);
  assert.throws(() => planCacheDeletions([null]), /is not an object/);
  assert.throws(() => planCacheDeletions([entry({ id: 0 })]), /usable id/);
  assert.throws(
    () => planCacheDeletions([entry({ sizeInBytes: undefined })]),
    /usable size/,
  );
  assert.throws(
    () => planCacheDeletions([entry({ sizeInBytes: -1 })]),
    /usable size/,
  );
  assert.throws(
    () => planCacheDeletions([entry({ createdAt: "not a date" })]),
    /usable creation time/,
  );
  assert.throws(
    () => planCacheDeletions([entry({ lastAccessedAt: "not a date" })]),
    /usable last-accessed time/,
  );
  assert.throws(() => planCacheDeletions([entry({ ref: "" })]), /usable ref/);
  assert.throws(
    () =>
      planCacheDeletions([
        entry({ id: 1 }),
        entry({ id: 1, key: "another-family" }),
      ]),
    /duplicate cache id 1/,
  );
  assert.throws(
    () => planCacheDeletions([], { budgetBytes: 0 }),
    /budgetBytes must be a positive number/,
  );
  assert.throws(
    () => planCacheDeletions([], { idleMinutes: -1 }),
    /idleMinutes must be zero or a positive number/,
  );
  assert.throws(
    () => planCacheDeletions([], { now: Number.NaN }),
    /now must be a finite timestamp/,
  );
  assert.throws(
    () => planCacheDeletions([], { defaultRef: "" }),
    /defaultRef must be a non-empty ref/,
  );
  assert.throws(
    () => planCacheDeletions([], { keep: 0 }),
    /keep must be a positive integer/,
  );
  assert.throws(
    () => planCacheDeletions([], { retention: "v0-rust-" }),
    /retention must be an array/,
  );
  assert.throws(
    () =>
      planCacheDeletions([], {
        retention: [{ prefix: "v0-rust-", keep: 0 }],
      }),
    /retention rule 0 is invalid/,
  );
});

test("the same listing always plans the same deletions", () => {
  const first = planCacheDeletions(OBSERVED, {
    now: OBSERVED_AT,
    openPullRequests: [199],
  });
  const second = planCacheDeletions([...OBSERVED].reverse(), {
    now: OBSERVED_AT,
    openPullRequests: [199],
  });
  assert.deepEqual(
    first.deletions.map((deletion) => deletion.id),
    second.deletions.map((deletion) => deletion.id),
  );
});

// The measurement in #182: the installer's dependency cache was evicted
// between two runs an hour apart because the repository sat over the 10 GiB
// cap, and the resulting cold compile of all 395 crates is the difference
// between a 13.30 and a 20.73 minute post-merge envelope.
test("the 2026-09-23 listing comes back under the cap without touching a live cache", () => {
  const plan = planCacheDeletions(OBSERVED, {
    now: OBSERVED_AT,
    openPullRequests: [199],
  });

  assert.equal(plan.totalBytes, sumFixture());
  assert.ok(
    plan.totalBytes > 10 * GIB,
    "the observed listing was over the cap",
  );

  // Everything the merged pull request 198 left behind, and nothing else.
  assert.deepEqual(
    plan.deletions.map((deletion) => [deletion.id, deletion.reason]),
    [
      [8003454638, "closed-pull-request"],
      [8003477963, "closed-pull-request"],
      [8003511373, "closed-pull-request"],
      [8003575878, "closed-pull-request"],
    ],
  );
  assert.equal(plan.freedBytes, 1_426_095_631);
  assert.ok(plan.remainingBytes < 9 * GIB);
  assert.equal(plan.overBudget, false);
  assert.equal(plan.structurallyOverBudget, false);

  // The entry whose eviction #182 measured stays, as does every other
  // default-branch cache and the open pull request's own scope.
  const deletedIds = new Set(plan.deletions.map((deletion) => deletion.id));
  for (const cache of OBSERVED) {
    if (
      cache.ref === "refs/heads/main" ||
      cache.ref === "refs/pull/199/merge"
    ) {
      assert.equal(deletedIds.has(cache.id), false, cache.key);
    }
  }
});

test("with no pull-request listing the same run still frees the superseded generations", () => {
  // The workflow's `gh pr list` can fail. Without it no scope can be proven
  // dead, so the same four entries are reclaimed by the budget rail instead -
  // least recently used first, and only as many as the budget needs.
  const plan = planCacheDeletions(OBSERVED, { now: OBSERVED_AT });
  for (const deletion of plan.deletions) {
    assert.equal(deletion.reason, "budget");
  }
  assert.deepEqual(
    plan.deletions.map((deletion) => deletion.id),
    [8003454638, 8003477963, 8003511373, 8003575878],
  );
  assert.equal(plan.overBudget, false);
  // The open pull request's own newest entry is the last thing standing
  // between the budget and the shared caches, and it is never reached here.
  assert.ok(
    plan.deletions.every((deletion) => deletion.ref !== "refs/pull/199/merge"),
  );
});

// GitHub's cap is the failure condition; the 9 GiB budget is a headroom
// target. The four tests below are the evidence for separating them.

// (1) The false positive. This listing is the steady state of the repository
// after a correct, complete prune, and it must not report failure.
test("a listing pruned to its irreducible floor warns but does not fail", () => {
  const plan = planCacheDeletions(SATURATED, {
    now: SATURATED_AT,
    openPullRequests: [],
  });

  assert.ok(
    plan.totalBytes > 10 * GIB,
    "the observed listing was over the cap",
  );
  assert.equal(plan.totalBytes, 12_773_298_537);

  // Everything safely reclaimable goes: the merged pull request 199 scope and
  // the three superseded `aur-sccache-v1` generations.
  assert.deepEqual(
    [...new Set(plan.deletions.map((deletion) => deletion.reason))].sort(),
    ["closed-pull-request", "superseded"],
  );
  assert.equal(plan.deletions.length, 6);

  // Under GitHub's cap, which is the whole objective of #182 cause (2)...
  assert.ok(plan.remainingBytes < DEFAULT_CAP_BYTES);
  assert.equal(plan.overCap, false);
  assert.equal(plan.structurallyOverCap, false);

  // ...but still over the headroom target, which is a warning, not a failure.
  assert.equal(plan.overBudget, true);
  assert.equal(plan.structurallyOverBudget, true);

  const report = renderPlan(plan);
  assert.doesNotMatch(report, /ERROR/);
  assert.match(report, /WARNING: .*headroom target/);

  // No default-branch entry that is still the only generation of its family
  // is touched, so no job loses a cache hit to this prune.
  const deleted = new Set(plan.deletions.map((deletion) => deletion.id));
  for (const cache of SATURATED) {
    if (cache.ref === "refs/heads/main" && cache.key.startsWith("v0-rust-")) {
      assert.equal(deleted.has(cache.id), false, cache.key);
    }
  }
});

// (2) Causality. Without the split, the condition the workflow used to fail on
// is exactly the one this listing trips, so the job would have been red from
// its first run.
test("the pre-split condition would have failed on that same listing", () => {
  const plan = planCacheDeletions(SATURATED, {
    now: SATURATED_AT,
    openPullRequests: [],
  });
  assert.equal(plan.overBudget, true);
  assert.equal(plan.overCap, false);
  assert.notEqual(plan.overBudget, plan.overCap);

  // And the reason is irreducible rather than a prune that gave up early: the
  // single-generation `v0-rust-*` families alone are most of the remainder.
  const rustBytes = SATURATED.filter(
    (cache) =>
      cache.ref === "refs/heads/main" && cache.key.startsWith("v0-rust-"),
  ).reduce((total, cache) => total + cache.sizeInBytes, 0);
  assert.ok(rustBytes > 7 * GIB, `v0-rust families were ${rustBytes} bytes`);
  assert.ok(plan.remainingBytes - rustBytes < 2 * GIB);
});

// (3) A real overrun still fails. Raising the floor above the cap must report
// an error, not a softened warning.
test("a retained set that does not fit under the cap is still an error", () => {
  const plan = planCacheDeletions(
    [
      entry({ id: 1, sizeInBytes: 6 * GIB }),
      entry({
        id: 2,
        key: "v0-rust-rust-ubuntu-Linux-x64-6ff13d87-aea8b408",
        sizeInBytes: 5 * GIB,
      }),
    ],
    { now: NOW },
  );
  assert.deepEqual(plan.deletions, []);
  assert.equal(plan.overCap, true);
  assert.equal(plan.structurallyOverCap, true);
  assert.match(renderPlan(plan), /ERROR: .*over GitHub's 10\.00 GiB cap/);
});

test("over the cap but reducible is an error about the prune, not the policy", () => {
  const plan = planCacheDeletions(
    [
      entry({ id: 1, sizeInBytes: 6 * GIB }),
      entry({
        id: 2,
        key: "webview-binary-linux-0000000000000000000000000000000a",
        ref: "refs/pull/199/merge",
        sizeInBytes: 5 * GIB,
        lastAccessedAt: "2026-09-23T11:59:00Z",
      }),
    ],
    { now: NOW, openPullRequests: [199] },
  );
  // The only reclaimable entry is inside the idle window, so this pass really
  // is still over the cap and must say so.
  assert.deepEqual(plan.deletions, []);
  assert.equal(plan.overCap, true);
  assert.equal(plan.structurallyOverCap, false);
  assert.match(renderPlan(plan), /ERROR: still 1\.00 GiB over GitHub's/);
});

// (4) Adjacent behaviour pinned. Adding the cap must not have moved which
// entries get deleted, and a budget above the cap must be refused rather than
// silently disabling the only error condition.
test("the cap changes severity only, never the planned deletions", () => {
  const base = planCacheDeletions(OBSERVED, {
    now: OBSERVED_AT,
    openPullRequests: [199],
  });
  const tighter = planCacheDeletions(OBSERVED, {
    now: OBSERVED_AT,
    openPullRequests: [199],
    capBytes: 10 * GIB,
  });
  assert.deepEqual(tighter.deletions, base.deletions);
  assert.equal(tighter.freedBytes, base.freedBytes);
  assert.equal(tighter.remainingBytes, base.remainingBytes);
  assert.equal(base.capBytes, DEFAULT_CAP_BYTES);
  assert.equal(base.budgetBytes, DEFAULT_BUDGET_BYTES);
  assert.ok(DEFAULT_BUDGET_BYTES < DEFAULT_CAP_BYTES);
});

test("a budget above the cap is refused instead of disabling the error", () => {
  assert.throws(
    () => planCacheDeletions([], { budgetBytes: 11 * GIB, capBytes: 10 * GIB }),
    /budgetBytes must not exceed capBytes/,
  );
  for (const capBytes of [0, -1, Number.NaN, "10"]) {
    assert.throws(
      () => planCacheDeletions([], { capBytes }),
      /capBytes must be a positive number/,
      `capBytes ${String(capBytes)}`,
    );
  }
});

test("the workflow prunes off the critical path and fails closed on incomplete work", () => {
  const workflow = readFileSync(
    new URL("../../.github/workflows/cache-budget.yml", import.meta.url),
    "utf8",
  );

  assert.match(workflow, /workflow_run:/);
  assert.match(workflow, /schedule:/);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /group: actions-cache-budget/);
  assert.match(workflow, /cancel-in-progress: false/);
  assert.match(workflow, /actions: write/);
  assert.match(workflow, /pull-requests: read/);
  assert.match(workflow, /inputs\['dry-run'\] != true/);
  assert.match(workflow, /--method DELETE/);
  assert.match(workflow, /actions\/caches\/\$\{id\}/);
  assert.match(workflow, /if \(\( failures > 0 \)\); then\s+exit 1/);
  // The cap is the failure condition and the headroom target is a warning.
  // Pinned in both directions so the two cannot quietly swap back.
  assert.match(workflow, /if \(plan\.overCap\)/);
  assert.match(workflow, /if \(plan\.overBudget\)/);
  assert.match(workflow, /::error::.*\$\{plan\.capBytes\}/);
  assert.match(workflow, /::warning::.*\$\{plan\.budgetBytes\}/);
  assert.doesNotMatch(workflow, /\bon:\s*\[(?:pull_request|push)\]/);
});

function sumFixture() {
  return OBSERVED.reduce((total, cache) => total + cache.sizeInBytes, 0);
}
