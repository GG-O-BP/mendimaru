// Issue #182. Runs every `*.node-test.mjs` under the given directories.
//
// The point is not convenience. `package.json` is a build-fingerprint input
// (`scripts/perf/build-fingerprint.mjs`), so editing it invalidates the
// installer cache and forces a full cold Rust rebuild on the post-merge path.
// Listing test files by hand inside an npm script meant that adding one test
// file - a change that cannot affect a built installer - cost a rebuild. Merge
// `1be34f4` did exactly that: its only fingerprint-relevant diff was one line
// in the `scripts` block, and the candidate installer build went from a 24 s
// cache hit to a 566 s compile.
//
// A bare shell glob cannot replace the hand-written list, because `node --test`
// exits 0 after reporting zero tests when a pattern matches nothing. That would
// turn a renamed convention into a silently passing suite. This runner refuses
// to start when a requested directory contributes no test file.
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const TEST_FILE_SUFFIX = ".node-test.mjs";

export function collectTestFiles(directories, readDirectory = readdirSync) {
  if (!Array.isArray(directories) || directories.length === 0) {
    throw new Error(
      "usage: node scripts/ci/node-test-suite.mjs <directory> [directory...]",
    );
  }
  const files = [];
  for (const directory of directories) {
    const found = readDirectory(directory)
      .filter((entry) => entry.endsWith(TEST_FILE_SUFFIX))
      .sort()
      .map((entry) => path.posix.join(directory, entry));
    if (found.length === 0) {
      throw new Error(
        `${directory} contains no ${TEST_FILE_SUFFIX} file; refusing to report an empty suite as a pass`,
      );
    }
    files.push(...found);
  }
  return files;
}

function main() {
  const files = collectTestFiles(process.argv.slice(2));
  const result = spawnSync(process.execPath, ["--test", ...files], {
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.signal) {
    throw new Error(`node --test terminated on ${result.signal}`);
  }
  process.exit(result.status ?? 1);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main();
}
