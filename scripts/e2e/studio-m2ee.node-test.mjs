import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

test("Studio M2EE verifier rejects HTTP 200 authentication denials", () => {
  const command = process.platform === "win32" ? "python" : "python3";
  const result = spawnSync(
    command,
    [fileURLToPath(new URL("./verify-studio-m2ee.test.py", import.meta.url))],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stdout + result.stderr);
});
