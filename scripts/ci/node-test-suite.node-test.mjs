import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import test from "node:test";

import { collectTestFiles, TEST_FILE_SUFFIX } from "./node-test-suite.mjs";

test("every suite directory is expanded in a stable order", () => {
  const readDirectory = (directory) =>
    directory === "scripts/ci"
      ? ["z.node-test.mjs", "a.node-test.mjs", "helper.mjs", "notes.md"]
      : ["only.node-test.mjs"];

  assert.deepEqual(
    collectTestFiles(["scripts/ci", "scripts/perf"], readDirectory),
    [
      "scripts/ci/a.node-test.mjs",
      "scripts/ci/z.node-test.mjs",
      "scripts/perf/only.node-test.mjs",
    ],
  );
});

// The whole reason this runner exists instead of a bare shell glob: a pattern
// that matches nothing makes `node --test` report zero tests and exit 0, which
// would turn a renamed convention into a silently passing suite.
test("a directory with no test file is refused rather than reported as a pass", () => {
  assert.throws(
    () => collectTestFiles(["scripts/ci"], () => ["gate-reports.mjs"]),
    /contains no \.node-test\.mjs file/,
  );
});

test("an empty directory list is refused", () => {
  assert.throws(() => collectTestFiles([]), /usage:/);
  assert.throws(() => collectTestFiles(undefined), /usage:/);
});

// Pins the sets the npm scripts used to spell out by hand, so replacing the
// hand-written lists cannot quietly drop or add a suite.
test("the real suite directories expand to the files the npm scripts listed", () => {
  assert.deepEqual(collectTestFiles(["scripts/ci"], readdirSync), [
    "scripts/ci/gate-relevance.node-test.mjs",
    "scripts/ci/gate-reports.node-test.mjs",
    "scripts/ci/node-test-suite.node-test.mjs",
    "scripts/ci/regression-hold.node-test.mjs",
    "scripts/ci/regression-issue.node-test.mjs",
  ]);
  assert.deepEqual(collectTestFiles(["scripts/perf"], readdirSync), [
    "scripts/perf/build-fingerprint.node-test.mjs",
    "scripts/perf/measurement-failure.node-test.mjs",
    "scripts/perf/performance-core.node-test.mjs",
    "scripts/perf/release-relevance.node-test.mjs",
    "scripts/perf/webview-driver.node-test.mjs",
  ]);
  assert.equal(TEST_FILE_SUFFIX, ".node-test.mjs");
});
