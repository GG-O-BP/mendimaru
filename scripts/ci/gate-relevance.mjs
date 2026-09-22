import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { tauriResourceInputs } from "../perf/release-relevance.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

// Every gate re-runs when the workflow or the classifier itself changes. A
// change to the relevance rules can therefore never skip the gate it governs,
// and a reviewer never has to reason about a half-applied path contract.
const sharedFiles = [
  ".github/workflows/ci.yml",
  ".npmrc",
  "package-lock.json",
  "package.json",
  "rust-toolchain",
  "rust-toolchain.toml",
];

const sharedPrefixes = ["scripts/ci/"];

// Inputs that change the application the three product gates install, launch,
// and drive. Kept separate from the per-gate contracts because all three share
// it and drift between them would be silent.
const productFiles = [
  "index.html",
  "tsconfig.json",
  "tsconfig.node.json",
  "vite.config.ts",
];

const productPrefixes = [
  ".cargo/",
  "public/",
  "schemas/",
  "src/",
  "src-tauri/",
];

// One contract per gated job. `product` opts a gate into the shared
// application inputs above; the dependency audit deliberately stays out of it
// so that ordinary product changes do not re-run a 213 second audit that only
// reads dependency manifests and the RustSec policy.
const gateContracts = {
  aur: {
    job: "aur-package",
    files: [".github/workflows/aur-package.yml"],
    prefixes: ["aur/", "scripts/aur/"],
    product: true,
  },
  bundle: {
    job: "windows-bundle",
    files: [
      "scripts/e2e/windows-bundle-smoke.ps1",
      "scripts/e2e/windows-bundle-smoke.test.ps1",
    ],
    prefixes: [],
    product: true,
  },
  windows_e2e: {
    job: "windows-native-e2e",
    files: [],
    prefixes: ["scripts/e2e/"],
    product: true,
  },
  linux_e2e: {
    job: "linux-webkit-e2e",
    files: ["scripts/test-tauri-e2e.mjs", "scripts/verify-e2e-coverage.mjs"],
    prefixes: ["scripts/e2e/"],
    product: true,
  },
  security: {
    job: "security",
    files: [
      "scripts/rustsec-policy.node-test.mjs",
      "scripts/verify-rustsec-policy.mjs",
      "src-tauri/Cargo.lock",
      "src-tauri/Cargo.toml",
    ],
    prefixes: ["actions/dependency-audit/", "security/"],
    product: false,
  },
};

export const ciGateNames = Object.freeze(Object.keys(gateContracts));

export function ciGateJobName(gate) {
  return gateContracts[gate]?.job;
}

function resolveContract(name) {
  const contract = gateContracts[name];
  const files = new Set([...sharedFiles, ...contract.files]);
  const prefixes = [...sharedPrefixes, ...contract.prefixes];
  if (contract.product) {
    for (const file of productFiles) files.add(file);
    prefixes.push(...productPrefixes);
    // Anything packaged into the application by Tauri changes what the gate
    // installs, even when it lives outside src/ or src-tauri/.
    for (const resource of tauriResourceInputs()) files.add(resource);
  }
  return { files, prefixes };
}

const resolvedContracts = new Map(
  ciGateNames.map((name) => [name, resolveContract(name)]),
);

export function normalizeChangedPath(input) {
  return String(input)
    .replaceAll("\\", "/")
    .replace(/^\.\/+/, "");
}

export function isCiGateRelevantPath(gate, input) {
  const contract = resolvedContracts.get(gate);
  // An unrecognised gate has no contract to prove irrelevance with, so it must
  // never be reported as skippable.
  if (!contract) return true;
  const normalized = normalizeChangedPath(input);
  // Git never emits empty entries, parent traversal, or absolute paths from
  // `diff --name-only -z`. Treat anything malformed as relevant rather than
  // risk skipping a required gate on input we do not understand.
  if (
    normalized === "" ||
    normalized.startsWith("../") ||
    normalized.startsWith("/") ||
    /^[A-Za-z]:\//.test(normalized)
  ) {
    return true;
  }
  return (
    contract.files.has(normalized) ||
    contract.prefixes.some((prefix) => normalized.startsWith(prefix))
  );
}

export function ciGateRelevance(gate, paths) {
  // An empty change set means the diff did not describe the pull request the
  // way we expect. Run the gate rather than trust it.
  if (paths.length === 0) return { relevant: true, relevantPaths: [] };
  const relevantPaths = paths.filter((entry) =>
    isCiGateRelevantPath(gate, entry),
  );
  return { relevant: relevantPaths.length > 0, relevantPaths };
}

export function allCiGateRelevance(paths) {
  return Object.fromEntries(
    ciGateNames.map((gate) => [gate, ciGateRelevance(gate, paths)]),
  );
}

export function changedPaths(base, head, { cwd = repository } = {}) {
  // --no-renames lists the removed and the added path separately, so a rename
  // into or out of a gated directory is visible to both contracts. Deletions
  // stay listed under their original path.
  const output = execFileSync(
    "git",
    ["diff", "--name-only", "--no-renames", "-z", base, head, "--"],
    { cwd },
  );
  return output.toString("utf8").split("\0").filter(Boolean);
}

function parseArguments(arguments_) {
  const values = {};
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      throw new Error(`invalid ci gate relevance argument: ${name ?? ""}`);
    }
    values[name.slice(2)] = value;
  }
  if (!values.base) throw new Error("--base is required");
  return {
    base: values.base,
    head: values.head ?? "HEAD",
    githubOutput: values["github-output"],
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const options = parseArguments(process.argv.slice(2));
  const paths = changedPaths(options.base, options.head);
  const relevance = allCiGateRelevance(paths);
  process.stdout.write(`CI gate changed paths: ${JSON.stringify(paths)}\n`);
  for (const gate of ciGateNames) {
    const { relevant, relevantPaths } = relevance[gate];
    process.stdout.write(
      `CI gate ${gate} (${ciGateJobName(gate)}): ${relevant} ${JSON.stringify(
        relevantPaths,
      )}\n`,
    );
  }
  if (options.githubOutput) {
    // Written once, after every gate has been decided, so a mid-run failure
    // cannot leave a partially populated output file behind for the caller to
    // misread as a complete decision.
    appendFileSync(
      options.githubOutput,
      ciGateNames
        .map(
          (gate) => `${gate}=${relevance[gate].relevant ? "true" : "false"}\n`,
        )
        .join(""),
    );
  }
}
