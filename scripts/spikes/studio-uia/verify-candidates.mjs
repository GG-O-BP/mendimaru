import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, open, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Offline verification only. It never downloads, extracts or executes a file.
export async function verifyCandidate(directory, artifact) {
  assert.match(artifact.file, /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,99}$/);
  assert.match(artifact.sha256, /^[a-f0-9]{64}$/);
  assert(
    Number.isSafeInteger(artifact.bytes) &&
      artifact.bytes > 0 &&
      artifact.bytes <= 128 * 1024 * 1024,
    "invalid candidate size",
  );
  const filename = path.join(directory, artifact.file);
  assert((await lstat(filename)).isFile(), "candidate must be a regular file");
  const file = await open(
    filename,
    constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0),
  );
  try {
    const metadata = await file.stat();
    assert(metadata.isFile(), "candidate must be a regular file");
    assert.equal(metadata.size, artifact.bytes, "candidate size mismatch");
    const hash = createHash("sha256");
    const buffer = Buffer.alloc(64 * 1024);
    let bytes = 0;
    while (bytes <= artifact.bytes) {
      const result = await file.read(buffer, 0, buffer.length, null);
      if (result.bytesRead === 0) break;
      bytes += result.bytesRead;
      assert(bytes <= artifact.bytes, "candidate grew during verification");
      hash.update(buffer.subarray(0, result.bytesRead));
    }
    assert.equal(
      bytes,
      artifact.bytes,
      "candidate changed during verification",
    );
    assert.equal(
      hash.digest("hex"),
      artifact.sha256,
      "candidate hash mismatch",
    );
    return { file: artifact.file, bytes, status: "verified" };
  } finally {
    await file.close();
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  assert.equal(
    process.argv.length,
    3,
    "usage: verify-candidates.mjs DIRECTORY",
  );
  const manifest = JSON.parse(
    await readFile(new URL("./candidates.json", import.meta.url), "utf8"),
  );
  assert.equal(manifest.schemaVersion, 1);
  const results = [];
  for (const artifact of manifest.artifacts) {
    results.push(await verifyCandidate(process.argv[2], artifact));
  }
  console.log(
    JSON.stringify({ schemaVersion: 1, artifacts: results }, null, 2),
  );
}
