import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

const relevantPrefixes = [
  ".cargo/",
  "performance/",
  "public/",
  "scripts/perf/",
  "schemas/performance-",
  "src/",
  "src-tauri/",
];

const relevantFiles = new Set([
  ".github/workflows/release-performance.yml",
  ".npmrc",
  "index.html",
  "package-lock.json",
  "package.json",
  "rust-toolchain",
  "rust-toolchain.toml",
  "scripts/e2e/windows-bundle-smoke.ps1",
  "tsconfig.json",
  "tsconfig.node.json",
  "vite.config.ts",
]);

for (const resource of tauriResourceInputs()) relevantFiles.add(resource);

export function isReleasePerformanceRelevantPath(input) {
  const normalized = input.replaceAll("\\", "/").replace(/^\.\/+/, "");
  // Git never returns parent traversal or absolute paths. Treat malformed
  // input as relevant rather than risk silently skipping a measurement.
  if (
    normalized.startsWith("../") ||
    normalized.startsWith("/") ||
    /^[A-Za-z]:\//.test(normalized)
  ) {
    return true;
  }
  return (
    relevantFiles.has(normalized) ||
    relevantPrefixes.some((prefix) => normalized.startsWith(prefix))
  );
}

export function releasePerformanceRelevance(paths) {
  const relevant = paths.filter(isReleasePerformanceRelevantPath);
  return { relevant: relevant.length > 0, relevantPaths: relevant };
}

function changedPaths(base, head) {
  const output = execFileSync(
    "git",
    ["diff", "--name-only", "--no-renames", "-z", base, head, "--"],
    { cwd: repository },
  );
  return output.toString("utf8").split("\0").filter(Boolean);
}

function tauriResourceInputs() {
  const config = JSON.parse(
    readFileSync(path.join(repository, "src-tauri", "tauri.conf.json"), "utf8"),
  );
  const resources = config.bundle?.resources ?? {};
  const inputs = Array.isArray(resources) ? resources : Object.keys(resources);
  return inputs
    .filter((resource) => !resource.includes("node_modules"))
    .map((resource) =>
      path
        .relative(repository, path.resolve(repository, "src-tauri", resource))
        .replaceAll(path.sep, "/"),
    );
}

function parseArguments(arguments_) {
  const values = {};
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      throw new Error(`invalid release relevance argument: ${name ?? ""}`);
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
  const result = releasePerformanceRelevance(paths);
  process.stdout.write(
    `Release-performance changed paths: ${JSON.stringify(paths)}\n`,
  );
  process.stdout.write(
    `Release-performance relevant paths: ${JSON.stringify(
      result.relevantPaths,
    )}\n`,
  );
  process.stdout.write(`Measured-artifact relevance: ${result.relevant}\n`);
  if (options.githubOutput) {
    appendFileSync(
      options.githubOutput,
      `relevant=${result.relevant ? "true" : "false"}\n`,
    );
  }
}
