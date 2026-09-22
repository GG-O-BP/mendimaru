import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  allCiGateRelevance,
  ciGateJobName,
  ciGateNames,
  ciGateRelevance,
  isCiGateRelevantPath,
  normalizeChangedPath,
} from "./gate-relevance.mjs";

import { tauriResourceInputs } from "../perf/release-relevance.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

const workflow = readFileSync(
  path.join(repository, ".github", "workflows", "ci.yml"),
  "utf8",
);

const productGates = ["aur", "bundle", "windows_e2e", "linux_e2e"];

test("every gate name maps to a real ci.yml job", () => {
  for (const gate of ciGateNames) {
    const job = ciGateJobName(gate);
    assert.ok(job, `${gate} must name the job it governs`);
    assert.match(
      workflow,
      new RegExp(`^  ${job}:$`, "m"),
      `${job} must exist in ci.yml`,
    );
  }
});

test("ci.yml consumes every gate this classifier decides", () => {
  for (const gate of ciGateNames) {
    assert.ok(
      workflow.includes(`needs.changes.outputs.${gate}`),
      `ci.yml must consume the ${gate} decision`,
    );
  }
});

test("the changes job declares an output for every gate", () => {
  const outputs = workflow.slice(
    workflow.indexOf("    outputs:"),
    workflow.indexOf("    steps:"),
  );
  for (const gate of ciGateNames) {
    assert.match(
      outputs,
      new RegExp(`^      ${gate}:`, "m"),
      `the changes job must export ${gate}`,
    );
  }
});

test("product changes run every product gate", () => {
  for (const input of [
    "src/App.tsx",
    "src-tauri/src/main.rs",
    "src-tauri/tauri.conf.json",
    "public/favicon.svg",
    "schemas/capabilities.schema.json",
    ".cargo/config.toml",
    "index.html",
    "vite.config.ts",
    "tsconfig.json",
    "tsconfig.node.json",
  ]) {
    for (const gate of productGates) {
      assert.equal(
        isCiGateRelevantPath(gate, input),
        true,
        `${input} must run ${ciGateJobName(gate)}`,
      );
    }
  }
});

test("every packaged Tauri resource runs the product gates", () => {
  for (const resource of tauriResourceInputs()) {
    for (const gate of productGates) {
      assert.equal(
        isCiGateRelevantPath(gate, resource),
        true,
        `${resource} is packaged into the application ${ciGateJobName(
          gate,
        )} installs`,
      );
    }
  }
});

test("the dependency audit tracks manifests and policy, not product code", () => {
  for (const input of [
    "package.json",
    "package-lock.json",
    ".npmrc",
    "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock",
    "security/rustsec-exceptions.json",
    "scripts/verify-rustsec-policy.mjs",
    "scripts/rustsec-policy.node-test.mjs",
    "actions/dependency-audit/action.yml",
  ]) {
    assert.equal(
      isCiGateRelevantPath("security", input),
      true,
      `${input} must run the dependency audit`,
    );
  }
  for (const input of [
    "src/App.tsx",
    "src-tauri/src/main.rs",
    "public/favicon.svg",
    "index.html",
  ]) {
    assert.equal(
      isCiGateRelevantPath("security", input),
      false,
      `${input} cannot change the audited dependency set`,
    );
  }
});

test("each product gate keeps the harness it actually runs", () => {
  assert.equal(
    isCiGateRelevantPath("bundle", "scripts/e2e/windows-bundle-smoke.ps1"),
    true,
  );
  assert.equal(
    isCiGateRelevantPath("bundle", "scripts/e2e/windows-bundle-smoke.test.ps1"),
    true,
  );
  assert.equal(
    isCiGateRelevantPath("windows_e2e", "scripts/e2e/windows-native.mjs"),
    true,
  );
  assert.equal(
    isCiGateRelevantPath("linux_e2e", "scripts/test-tauri-e2e.mjs"),
    true,
  );
  assert.equal(
    isCiGateRelevantPath("linux_e2e", "scripts/verify-e2e-coverage.mjs"),
    true,
  );
  assert.equal(
    isCiGateRelevantPath("aur", "aur/PKGBUILD"),
    true,
    "the AUR recipe must run its own gate",
  );
  assert.equal(
    isCiGateRelevantPath("aur", ".github/workflows/aur-package.yml"),
    true,
  );
});

test("the AUR contract still covers every path its inline predecessor did", () => {
  // The previous ci.yml filter matched these paths directly. Narrowing the
  // contract while moving it here would silently stop packaging runs.
  for (const input of [
    "aur/PKGBUILD",
    "scripts/aur/update.sh",
    "src/App.tsx",
    "src-tauri/Cargo.toml",
    "package.json",
    ".github/workflows/aur-package.yml",
  ]) {
    assert.equal(
      isCiGateRelevantPath("aur", input),
      true,
      `${input} ran the AUR gate before this classifier existed`,
    );
  }
});

test("workflow and classifier changes run every gate", () => {
  for (const input of [
    ".github/workflows/ci.yml",
    "scripts/ci/gate-relevance.mjs",
    "scripts/ci/gate-relevance.node-test.mjs",
    "package.json",
    "package-lock.json",
    "rust-toolchain.toml",
  ]) {
    for (const gate of ciGateNames) {
      assert.equal(
        isCiGateRelevantPath(gate, input),
        true,
        `${input} must re-run ${ciGateJobName(gate)}`,
      );
    }
  }
});

test("malformed and unrecognised input fails closed", () => {
  for (const gate of ciGateNames) {
    for (const input of [
      "",
      "../outside/file.ts",
      "/etc/passwd",
      "C:/Windows/system32/drivers/etc/hosts",
    ]) {
      assert.equal(
        isCiGateRelevantPath(gate, input),
        true,
        `${JSON.stringify(input)} must fail closed for ${gate}`,
      );
    }
  }
  assert.equal(
    isCiGateRelevantPath("not-a-gate", "docs/readme.md"),
    true,
    "an unknown gate has no contract and must never be skipped",
  );
});

test("an empty change set fails closed", () => {
  for (const gate of ciGateNames) {
    assert.deepEqual(ciGateRelevance(gate, []), {
      relevant: true,
      relevantPaths: [],
    });
  }
});

test("backslash and leading-dot paths normalize before matching", () => {
  assert.equal(normalizeChangedPath("./src/App.tsx"), "src/App.tsx");
  assert.equal(
    normalizeChangedPath("src-tauri\\src\\main.rs"),
    "src-tauri/src/main.rs",
  );
  assert.equal(isCiGateRelevantPath("bundle", "./src/App.tsx"), true);
  assert.equal(isCiGateRelevantPath("bundle", "src-tauri\\src\\main.rs"), true);
});

test("a documentation-only change skips every gate", () => {
  const relevance = allCiGateRelevance([
    "README.md",
    "docs/release-performance.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
  ]);
  for (const gate of ciGateNames) {
    assert.equal(
      relevance[gate].relevant,
      false,
      `${ciGateJobName(gate)} has nothing to verify in a docs-only change`,
    );
    assert.deepEqual(relevance[gate].relevantPaths, []);
  }
});

test("a rename out of a gated directory still runs that gate", () => {
  // --no-renames reports the removal and the addition separately, so the old
  // location alone has to be enough to trigger the gate.
  const relevance = allCiGateRelevance([
    "src-tauri/src/retired.rs",
    "docs/retired.md",
  ]);
  for (const gate of productGates) {
    assert.equal(relevance[gate].relevant, true);
    assert.deepEqual(relevance[gate].relevantPaths, [
      "src-tauri/src/retired.rs",
    ]);
  }
  assert.equal(relevance.security.relevant, false);
});

test("a deleted dependency manifest still runs the audit", () => {
  const relevance = allCiGateRelevance(["src-tauri/Cargo.lock"]);
  assert.equal(relevance.security.relevant, true);
});

test("every gate decision is a boolean for every gate name", () => {
  const relevance = allCiGateRelevance(["docs/readme.md", "src/App.tsx"]);
  assert.deepEqual(Object.keys(relevance).sort(), [...ciGateNames].sort());
  for (const gate of ciGateNames) {
    assert.equal(typeof relevance[gate].relevant, "boolean");
  }
});
