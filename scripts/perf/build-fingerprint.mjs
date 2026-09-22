import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { tauriResourceInputs } from "./release-relevance.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

// Only the inputs that can change the compiled artifact belong here. This is
// deliberately narrower than the release-relevance set: budgets, schemas,
// scripts/perf and the smoke script change what we measure, not what we
// build, so they must not invalidate an otherwise reusable binary. Anything
// that reaches the bundle must be listed, including Tauri resources that live
// outside src-tauri.
const buildInputTrees = [".cargo", "public", "src", "src-tauri"];

const buildInputFiles = [
  ".npmrc",
  "index.html",
  "package-lock.json",
  "package.json",
  "rust-toolchain",
  "rust-toolchain.toml",
  "tsconfig.json",
  "tsconfig.node.json",
  "vite.config.ts",
];

export function buildInputPaths({
  resourceInputs = tauriResourceInputs(),
} = {}) {
  const resources = resourceInputs.map((resource) => {
    const normalized = resource.replaceAll("\\", "/").replace(/^\.\/+/, "");
    if (
      !normalized ||
      normalized === ".." ||
      normalized.startsWith("../") ||
      normalized.startsWith("/") ||
      /^[A-Za-z]:\//.test(normalized)
    ) {
      throw new Error(
        `build fingerprint cannot safely hash Tauri resource outside the repository: ${resource}`,
      );
    }
    return normalized;
  });
  return [
    ...new Set([...buildInputTrees, ...buildInputFiles, ...resources]),
  ].sort();
}

// A tree object id already summarises every file underneath it, so one
// rev-parse per root covers the whole subtree exactly.
export function gitObjectId(commit, relativePath) {
  try {
    return execFileSync(
      "git",
      ["rev-parse", "--verify", "--quiet", `${commit}:${relativePath}`],
      { cwd: repository, stdio: ["ignore", "pipe", "ignore"] },
    )
      .toString("utf8")
      .trim();
  } catch (error) {
    // Optional inputs (rust-toolchain, .npmrc) are absent in most revisions.
    // Record the absence so adding the file later changes the fingerprint.
    if (error?.status === 1) return "absent";
    throw error;
  }
}

export function buildFingerprint({
  commit,
  salt = [],
  objectId = gitObjectId,
}) {
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(`build fingerprint requires a full commit sha: ${commit}`);
  }
  const digest = createHash("sha256");
  for (const input of buildInputPaths()) {
    digest.update(`path\u0000${input}\u0000${objectId(commit, input)}\n`);
  }
  for (const entry of [...salt].sort()) {
    digest.update(`salt\u0000${entry}\n`);
  }
  return digest.digest("hex").slice(0, 32);
}

function parseArguments(arguments_) {
  const values = { salt: [] };
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      throw new Error(`invalid build fingerprint argument: ${name ?? ""}`);
    }
    if (name === "--salt") {
      values.salt.push(value);
      continue;
    }
    values[name.slice(2)] = value;
  }
  if (!values.commit) throw new Error("--commit is required");
  return values;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const options = parseArguments(process.argv.slice(2));
  const fingerprint = buildFingerprint({
    commit: options.commit,
    salt: options.salt,
  });
  process.stdout.write(
    `Build fingerprint inputs: ${JSON.stringify(buildInputPaths())}\n`,
  );
  process.stdout.write(
    `Build fingerprint salt: ${JSON.stringify([...options.salt].sort())}\n`,
  );
  process.stdout.write(`Build fingerprint: ${fingerprint}\n`);
  if (options["github-output"]) {
    appendFileSync(options["github-output"], `fingerprint=${fingerprint}\n`);
  }
}
