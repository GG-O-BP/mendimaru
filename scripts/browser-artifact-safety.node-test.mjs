import assert from "node:assert/strict";
import test from "node:test";
import { strToU8, zipSync } from "fflate";
import {
  ArtifactSafetyError,
  BytePatternMatcher,
  DEFAULT_ARTIFACT_SAFETY_LIMITS,
  StreamingPatternScanner,
  inspectZipArchive,
  unzipArchiveBounded,
} from "./browser-artifact-safety.mjs";

const LOCAL_ENTRY = [0x50, 0x4b, 0x03, 0x04];
const CENTRAL_ENTRY = [0x50, 0x4b, 0x01, 0x02];

function safetyLimits(overrides = {}) {
  return {
    ...DEFAULT_ARTIFACT_SAFETY_LIMITS,
    maximumDurationMilliseconds: 5_000,
    ...overrides,
  };
}

function fixture(entries, options = { level: 6 }) {
  return zipSync(
    Object.fromEntries(
      Object.entries(entries).map(([name, value]) => [
        name,
        typeof value === "string" ? strToU8(value) : value,
      ]),
    ),
    options,
  );
}

function signatureOffsets(bytes, signature) {
  const offsets = [];
  outer: for (
    let offset = 0;
    offset <= bytes.length - signature.length;
    offset += 1
  ) {
    for (let index = 0; index < signature.length; index += 1) {
      if (bytes[offset + index] !== signature[index]) continue outer;
    }
    offsets.push(offset);
  }
  return offsets;
}

function view(bytes) {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
}

function expectKind(callback, kind) {
  assert.throws(
    callback,
    (error) => error instanceof ArtifactSafetyError && error.kind === kind,
  );
}

test("normal nested trace members pass bounded CRC-checked extraction", () => {
  const archive = fixture({
    "trace.trace": "normal trace",
    "resources/body.txt": "normal response body",
  });
  const inventory = inspectZipArchive(archive);
  assert.equal(inventory.size, 2);
  const extracted = unzipArchiveBounded(archive);
  assert.equal(
    new TextDecoder().decode(extracted["resources/body.txt"]),
    "normal response body",
  );
});

test("declared entry count, member, cumulative, and ratio limits are distinct", () => {
  const countArchive = fixture({ one: "1", two: "2", three: "3" });
  expectKind(
    () =>
      inspectZipArchive(countArchive, safetyLimits({ maximumZipEntries: 2 })),
    "zip_entry_count_limit",
  );
  expectKind(
    () =>
      inspectZipArchive(
        countArchive,
        safetyLimits({ maximumZipCentralDirectoryBytes: 1 }),
      ),
    "zip_central_directory_limit",
  );

  const memberArchive = fixture(
    { payload: new Uint8Array(1_025) },
    { level: 0 },
  );
  expectKind(
    () =>
      inspectZipArchive(
        memberArchive,
        safetyLimits({ maximumZipEntryBytes: 1_024 }),
      ),
    "zip_entry_size_limit",
  );

  const cumulativeArchive = fixture(
    { first: new Uint8Array(768), second: new Uint8Array(768) },
    { level: 0 },
  );
  expectKind(
    () =>
      inspectZipArchive(
        cumulativeArchive,
        safetyLimits({
          maximumZipEntryBytes: 1_024,
          maximumZipTotalBytes: 1_024,
        }),
      ),
    "zip_total_size_limit",
  );

  const ratioArchive = fixture({ repeated: new Uint8Array(32 * 1024) });
  expectKind(
    () =>
      inspectZipArchive(
        ratioArchive,
        safetyLimits({ maximumZipCompressionRatio: 2 }),
      ),
    "zip_compression_ratio_limit",
  );
});

test("a small compressed ZIP bomb is rejected before extraction", () => {
  const archive = fixture({ repeated: new Uint8Array(2 * 1024 * 1024) });
  assert.ok(archive.length < 16 * 1024);
  expectKind(
    () =>
      inspectZipArchive(
        archive,
        safetyLimits({ maximumZipEntryBytes: 1024 * 1024 }),
      ),
    "zip_entry_size_limit",
  );
});

test("encrypted, unsupported, unsafe-path, and truncated archives fail closed", () => {
  const original = fixture({ payload: "safe" }, { level: 0 });
  const local = signatureOffsets(original, LOCAL_ENTRY);
  const central = signatureOffsets(original, CENTRAL_ENTRY);
  assert.deepEqual([local.length, central.length], [1, 1]);

  const encrypted = Uint8Array.from(original);
  const encryptedView = view(encrypted);
  encryptedView.setUint16(
    local[0] + 6,
    encryptedView.getUint16(local[0] + 6, true) | 1,
    true,
  );
  encryptedView.setUint16(
    central[0] + 8,
    encryptedView.getUint16(central[0] + 8, true) | 1,
    true,
  );
  expectKind(() => inspectZipArchive(encrypted), "zip_unsupported_entry");

  const unsupported = Uint8Array.from(original);
  view(unsupported).setUint16(local[0] + 8, 12, true);
  view(unsupported).setUint16(central[0] + 10, 12, true);
  expectKind(() => inspectZipArchive(unsupported), "zip_unsupported_entry");

  expectKind(
    () => inspectZipArchive(fixture({ "../escape": "safe" })),
    "zip_unsafe_path",
  );
  expectKind(
    () => inspectZipArchive(original.subarray(0, original.length - 8)),
    "zip_malformed",
  );
});

test("declared size lies and CRC corruption are detected from actual bytes", () => {
  const original = fixture({ payload: new Uint8Array(2_048) }, { level: 0 });
  const local = signatureOffsets(original, LOCAL_ENTRY)[0];
  const central = signatureOffsets(original, CENTRAL_ENTRY)[0];

  const declaredSmall = Uint8Array.from(original);
  view(declaredSmall).setUint32(local + 22, 1, true);
  view(declaredSmall).setUint32(central + 24, 1, true);
  expectKind(
    () =>
      unzipArchiveBounded(declaredSmall, {
        limits: safetyLimits({ maximumZipEntryBytes: 1_024 }),
      }),
    "zip_entry_size_limit",
  );

  const badCrc = Uint8Array.from(original);
  const wrongCrc =
    (view(badCrc).getUint32(central + 16, true) ^ 0xffffffff) >>> 0;
  view(badCrc).setUint32(local + 14, wrongCrc, true);
  view(badCrc).setUint32(central + 16, wrongCrc, true);
  expectKind(() => unzipArchiveBounded(badCrc), "zip_declared_size_mismatch");
});

test("streaming secret matching covers names and chunk boundaries", () => {
  const secret = strToU8("boundary-private-value");
  const scanner = new StreamingPatternScanner([secret]);
  scanner.push(strToU8("safe-boundary-private-"));
  expectKind(() => scanner.push(strToU8("value-safe")), "private_value");
  assert.equal(scanner.bufferedBytes, 0);

  const matcher = new BytePatternMatcher(
    ["he", "she", "hers", "his"].map(strToU8),
  );
  assert.equal(matcher.contains(strToU8("ushers")), true);
  assert.equal(
    new TextDecoder().decode(matcher.redact(strToU8("ushers"))),
    "u*****",
  );

  const archive = fixture({ "resources/private-name.txt": "safe" });
  expectKind(
    () =>
      unzipArchiveBounded(archive, {
        collect: false,
        needles: [strToU8("private-name")],
      }),
    "private_entry_name",
  );
});

test("scan deadlines and errors do not disclose secret values", () => {
  const archive = fixture({ payload: "safe" });
  expectKind(
    () =>
      inspectZipArchive(
        archive,
        safetyLimits({ maximumDurationMilliseconds: 0 }),
      ),
    "scan_time_limit",
  );

  const secret = "do-not-disclose-this-value";
  let error;
  try {
    unzipArchiveBounded(fixture({ payload: secret }), {
      collect: false,
      needles: [strToU8(secret)],
    });
  } catch (caught) {
    error = caught;
  }
  assert.ok(error instanceof ArtifactSafetyError);
  assert.equal(error.kind, "private_value");
  assert.equal(error.message.includes(secret), false);
});
