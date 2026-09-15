import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { verifyCandidate } from "./verify-candidates.mjs";

test("verifies bytes and rejects corrupted, missing and oversized candidates without modifying them", async (t) => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "uia-candidate-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const content = Buffer.from("candidate fixture");
  const artifact = {
    file: "fixture.zip",
    bytes: content.length,
    sha256: createHash("sha256").update(content).digest("hex"),
  };
  const filename = path.join(directory, artifact.file);
  await writeFile(filename, content);
  assert.equal((await verifyCandidate(directory, artifact)).status, "verified");
  await assert.rejects(
    verifyCandidate(directory, { ...artifact, file: "missing.zip" }),
    { code: "ENOENT" },
  );
  await assert.rejects(
    verifyCandidate(directory, { ...artifact, bytes: 129 * 1024 * 1024 }),
    /invalid candidate size/,
  );
  await writeFile(filename, Buffer.alloc(content.length, 1));
  await assert.rejects(verifyCandidate(directory, artifact), /hash mismatch/);
  assert.deepEqual(await readFile(filename), Buffer.alloc(content.length, 1));
  await writeFile(filename, "too short");
  await assert.rejects(verifyCandidate(directory, artifact), /size mismatch/);
});

test("rejects traversal and directories", async (t) => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "uia-candidate-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const artifact = { file: "../secret", bytes: 1, sha256: "0".repeat(64) };
  await assert.rejects(verifyCandidate(directory, artifact));
  await assert.rejects(
    verifyCandidate(path.dirname(directory), {
      ...artifact,
      file: path.basename(directory),
    }),
    /regular file/,
  );
});

test(
  "rejects a symlink candidate",
  { skip: process.platform === "win32" },
  async (t) => {
    const directory = await mkdtemp(path.join(os.tmpdir(), "uia-candidate-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    await writeFile(path.join(directory, "original"), "a");
    await symlink("original", path.join(directory, "linked.zip"));
    await assert.rejects(
      verifyCandidate(directory, {
        file: "linked.zip",
        bytes: 1,
        sha256: createHash("sha256").update("a").digest("hex"),
      }),
      /regular file/,
    );
  },
);
