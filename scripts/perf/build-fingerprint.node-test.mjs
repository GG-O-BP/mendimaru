import assert from "node:assert/strict";
import test from "node:test";

import { buildFingerprint, buildInputPaths } from "./build-fingerprint.mjs";
import {
  isReleasePerformanceRelevantPath,
  tauriResourceInputs,
} from "./release-relevance.mjs";

const commit = "0".repeat(40);
const other = "1".repeat(40);

// A stub keeps the test independent of repository history: the contract under
// test is which paths are hashed and how, not what they contain today.
function stubObjectIds(overrides = {}) {
  return (_commit, relativePath) =>
    overrides[relativePath] ?? `oid-${relativePath}`;
}

test("every compiled input is fingerprinted", () => {
  const inputs = buildInputPaths();
  for (const required of [
    ".cargo",
    ".npmrc",
    "src",
    "src-tauri",
    "public",
    "index.html",
    "package.json",
    "package-lock.json",
    "vite.config.ts",
    "tsconfig.json",
    "tsconfig.node.json",
    "rust-toolchain",
    "rust-toolchain.toml",
  ]) {
    assert.ok(
      inputs.includes(required),
      `${required} changes the compiled artifact and must be fingerprinted`,
    );
  }
});

test("Tauri resources outside src-tauri are fingerprinted", () => {
  const inputs = buildInputPaths();
  for (const repositoryPath of tauriResourceInputs()) {
    if (repositoryPath.startsWith("src-tauri/")) continue;
    assert.ok(
      inputs.includes(repositoryPath),
      `${repositoryPath} is packaged into the binary and must be fingerprinted`,
    );
  }
});

test("Tauri resources outside the repository fail closed", () => {
  for (const resource of [
    "../outside-resource",
    "/outside-resource",
    "C:\\outside-resource",
  ]) {
    assert.throws(
      () => buildInputPaths({ resourceInputs: [resource] }),
      /outside the repository/,
      `${resource} cannot be represented by a Git object id`,
    );
  }
});

test("measurement-only inputs do not invalidate a reusable binary", () => {
  const inputs = buildInputPaths();
  // Directory roots are checked for absence only; the relevance classifier
  // receives file paths from git, never bare directory names.
  for (const measurementOnly of ["performance", "scripts/perf"]) {
    assert.ok(
      !inputs.includes(measurementOnly),
      `${measurementOnly} changes measurement, not the binary`,
    );
  }
  for (const measurementOnly of [
    "performance/budgets.latency.json",
    "schemas/performance-report.schema.json",
    "scripts/perf/release-webview.mjs",
    "scripts/e2e/windows-bundle-smoke.ps1",
    ".github/workflows/release-performance.yml",
  ]) {
    assert.ok(
      !inputs.includes(measurementOnly),
      `${measurementOnly} changes measurement, not the binary`,
    );
    assert.equal(
      isReleasePerformanceRelevantPath(measurementOnly),
      true,
      `${measurementOnly} must still force the suite to run`,
    );
  }
});

test("identical build inputs produce one shared key across revisions", () => {
  const salt = ["recipe=build --bundles msi,nsis", "rustc=1.0.0"];
  assert.equal(
    buildFingerprint({ commit, salt, objectId: stubObjectIds() }),
    buildFingerprint({ commit: other, salt, objectId: stubObjectIds() }),
  );
});

test("a changed compiled input produces a different key", () => {
  const salt = ["recipe=build"];
  const base = buildFingerprint({ commit, salt, objectId: stubObjectIds() });
  for (const changed of ["src", "src-tauri", "package-lock.json"]) {
    assert.notEqual(
      base,
      buildFingerprint({
        commit,
        salt,
        objectId: stubObjectIds({ [changed]: "changed" }),
      }),
      `${changed} must invalidate the cached binary`,
    );
  }
});

test("build flags and toolchain identity are part of the key", () => {
  const objectId = stubObjectIds();
  const base = buildFingerprint({
    commit,
    salt: ["recipe=build --bundles msi,nsis", "rustc=1.0.0"],
    objectId,
  });
  assert.notEqual(
    base,
    buildFingerprint({
      commit,
      salt: ["recipe=build --bundles msi", "rustc=1.0.0"],
      objectId,
    }),
    "changed build flags must invalidate the cached binary",
  );
  assert.notEqual(
    base,
    buildFingerprint({
      commit,
      salt: ["recipe=build --bundles msi,nsis", "rustc=1.0.1"],
      objectId,
    }),
    "a different rustc must invalidate the cached binary",
  );
});

test("runner image and architecture are part of the key", () => {
  const objectId = stubObjectIds();
  const base = buildFingerprint({
    commit,
    salt: ["runner-image=ubuntu24-20260901.1", "runner-arch=X64"],
    objectId,
  });
  assert.notEqual(
    base,
    buildFingerprint({
      commit,
      salt: ["runner-image=ubuntu24-20260908.1", "runner-arch=X64"],
      objectId,
    }),
    "a hosted-runner image update must invalidate native artifacts",
  );
  assert.notEqual(
    base,
    buildFingerprint({
      commit,
      salt: ["runner-image=ubuntu24-20260901.1", "runner-arch=ARM64"],
      objectId,
    }),
    "a different runner architecture must invalidate native artifacts",
  );
});

test("salt order does not change the key", () => {
  const objectId = stubObjectIds();
  assert.equal(
    buildFingerprint({ commit, salt: ["a=1", "b=2"], objectId }),
    buildFingerprint({ commit, salt: ["b=2", "a=1"], objectId }),
  );
});

test("an abbreviated revision is rejected rather than silently hashed", () => {
  assert.throws(
    () => buildFingerprint({ commit: "abc1234", objectId: stubObjectIds() }),
    /full commit sha/,
  );
});
