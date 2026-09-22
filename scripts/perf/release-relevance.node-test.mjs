import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import {
  isReleasePerformanceRelevantPath,
  releasePerformanceRelevance,
  tauriResourceInputs,
  tauriResourceInputsFromConfig,
} from "./release-relevance.mjs";

test("release inputs and measurement contracts remain relevant", () => {
  for (const input of [
    "src/App.tsx",
    "src-tauri/src/main.rs",
    "src-tauri/Cargo.lock",
    "src-tauri/tauri.e2e.conf.json",
    "scripts/perf/release-webview.mjs",
    "scripts/e2e/windows-bundle-smoke.ps1",
    "performance/budgets.latency.json",
    "schemas/performance-report.schema.json",
    "package.json",
    "package-lock.json",
    "vite.config.ts",
    "tsconfig.json",
    "tsconfig.node.json",
    "index.html",
    ".github/workflows/release-performance.yml",
    ".cargo/config.toml",
    "public/favicon.svg",
    "rust-toolchain.toml",
  ]) {
    assert.equal(
      isReleasePerformanceRelevantPath(input),
      true,
      `${input} must trigger full release measurement`,
    );
  }
});

test("every non-node Tauri resource remains relevant", () => {
  for (const repositoryPath of tauriResourceInputs()) {
    assert.equal(
      isReleasePerformanceRelevantPath(repositoryPath),
      true,
      `${repositoryPath} is packaged into the measured artifact`,
    );
  }
});

test("Tauri resource arrays and node_modules path segments are normalized", () => {
  const repositoryRoot = path.resolve("/repository");
  const configDirectory = path.join(repositoryRoot, "src-tauri");
  assert.deepEqual(
    tauriResourceInputsFromConfig(
      {
        bundle: {
          resources: [
            "../scripts/node_modules-audit.mjs",
            "../node_modules/package/index.js",
            "assets/runtime.json",
          ],
        },
      },
      { repositoryRoot, configDirectory },
    ),
    ["scripts/node_modules-audit.mjs", "src-tauri/assets/runtime.json"],
  );
});

test("unrelated automation, tests, and documentation use the fast skip path", () => {
  for (const input of [
    "scripts/aur/build-package.sh",
    "scripts/test-browser-e2e.mjs",
    "scripts/e2e/windows-native.mjs",
    "tests/browser/smoke.browser.json",
    ".github/workflows/ci.yml",
    ".github/CODEOWNERS",
    "docs/release-performance.md",
    "README.md",
    "eslint.config.js",
    "vitest.config.ts",
  ]) {
    assert.equal(
      isReleasePerformanceRelevantPath(input),
      false,
      `${input} cannot change a measured artifact or its gate`,
    );
  }
});

test("mixed path sets are relevant if any one input can affect measurement", () => {
  assert.deepEqual(
    releasePerformanceRelevance([
      "docs/release-performance.md",
      "scripts/aur/package-gate.sh",
    ]),
    { relevant: false, relevantPaths: [] },
  );
  assert.deepEqual(
    releasePerformanceRelevance([
      "docs/release-performance.md",
      "src/App.tsx",
      "README.md",
    ]),
    { relevant: true, relevantPaths: ["src/App.tsx"] },
  );
});

test("git path spelling is normalized and malformed paths fail safe", () => {
  assert.equal(isReleasePerformanceRelevantPath("./src/App.tsx"), true);
  assert.equal(isReleasePerformanceRelevantPath("src\\App.tsx"), true);
  assert.equal(isReleasePerformanceRelevantPath("../outside"), true);
  assert.equal(isReleasePerformanceRelevantPath("/outside"), true);
  assert.equal(isReleasePerformanceRelevantPath("C:\\outside"), true);
});
