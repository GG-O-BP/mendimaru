import { Unzip, UnzipInflate } from "fflate";

export const ARTIFACT_SCAN_BUFFER_BYTES = 64 * 1024;
export const DEFAULT_ARTIFACT_SAFETY_LIMITS = Object.freeze({
  maximumFileBytes: 512 * 1024 * 1024,
  maximumZipEntries: 4_096,
  maximumZipCentralDirectoryBytes: 8 * 1024 * 1024,
  maximumZipEntryBytes: 64 * 1024 * 1024,
  maximumZipTotalBytes: 256 * 1024 * 1024,
  maximumZipCompressionRatio: 200,
  maximumZipEntryNameBytes: 1_024,
  maximumZipPathComponents: 32,
  maximumDurationMilliseconds: 30_000,
});

const ERROR_MESSAGES = Object.freeze({
  file_size_limit: "a browser artifact exceeds the scan file size limit",
  private_entry_name: "a trace entry name contains a private value",
  private_value: "a browser artifact contains a private value",
  scan_time_limit: "the browser artifact scan time limit was exceeded",
  zip_compression_ratio_limit:
    "a trace ZIP entry exceeds the compression ratio limit",
  zip_central_directory_limit:
    "the trace ZIP central directory exceeds the size limit",
  zip_declared_size_mismatch:
    "a trace ZIP entry does not match its declared size",
  zip_entry_count_limit: "the trace ZIP entry count limit was exceeded",
  zip_entry_size_limit: "a trace ZIP entry exceeds the size limit",
  zip_malformed: "the trace ZIP is malformed",
  zip_total_size_limit: "the trace ZIP cumulative size limit was exceeded",
  zip_unsafe_path: "a trace ZIP entry path is unsafe",
  zip_unsupported_entry: "the trace ZIP contains an unsupported entry",
});

const END_OF_CENTRAL_DIRECTORY = 0x06054b50;
const CENTRAL_DIRECTORY_ENTRY = 0x02014b50;
const LOCAL_FILE_ENTRY = 0x04034b50;
const ZIP64_VALUE_16 = 0xffff;
const ZIP64_VALUE_32 = 0xffffffff;
const UTF8_NAME_FLAG = 1 << 11;
const DATA_DESCRIPTOR_FLAG = 1 << 3;
const ENCRYPTED_FLAGS = (1 << 0) | (1 << 6);
const STORED = 0;
const DEFLATED = 8;

export class ArtifactSafetyError extends Error {
  constructor(kind) {
    super(ERROR_MESSAGES[kind] || "the browser artifact is unsafe");
    this.name = "ArtifactSafetyError";
    this.kind = kind;
  }
}

export function inspectZipArchive(
  input,
  limits = DEFAULT_ARTIFACT_SAFETY_LIMITS,
  startedAt = Date.now(),
) {
  const bytes = asBytes(input);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const endOffset = findEndOfCentralDirectory(view);
  const disk = readUint16(view, endOffset + 4);
  const centralDisk = readUint16(view, endOffset + 6);
  const entriesOnDisk = readUint16(view, endOffset + 8);
  const entryCount = readUint16(view, endOffset + 10);
  const centralSize = readUint32(view, endOffset + 12);
  const centralOffset = readUint32(view, endOffset + 16);
  const commentLength = readUint16(view, endOffset + 20);
  if (
    disk !== 0 ||
    centralDisk !== 0 ||
    entriesOnDisk !== entryCount ||
    entryCount === ZIP64_VALUE_16 ||
    centralSize === ZIP64_VALUE_32 ||
    centralOffset === ZIP64_VALUE_32 ||
    endOffset + 22 + commentLength !== bytes.length ||
    centralOffset + centralSize !== endOffset
  ) {
    throw new ArtifactSafetyError("zip_unsupported_entry");
  }
  if (entryCount > limits.maximumZipEntries) {
    throw new ArtifactSafetyError("zip_entry_count_limit");
  }
  if (centralSize > limits.maximumZipCentralDirectoryBytes) {
    throw new ArtifactSafetyError("zip_central_directory_limit");
  }

  const inventory = new Map();
  const ranges = [];
  let declaredTotal = 0;
  let position = centralOffset;
  for (let index = 0; index < entryCount; index += 1) {
    ensureScanTime(startedAt, limits);
    if (readUint32(view, position) !== CENTRAL_DIRECTORY_ENTRY) {
      throw new ArtifactSafetyError("zip_malformed");
    }
    const madeBy = readUint16(view, position + 4);
    const flags = readUint16(view, position + 8);
    const compression = readUint16(view, position + 10);
    const crc32 = readUint32(view, position + 16);
    const compressedSize = readUint32(view, position + 20);
    const uncompressedSize = readUint32(view, position + 24);
    const nameLength = readUint16(view, position + 28);
    const extraLength = readUint16(view, position + 30);
    const entryCommentLength = readUint16(view, position + 32);
    const diskStart = readUint16(view, position + 34);
    const externalAttributes = readUint32(view, position + 38);
    const localOffset = readUint32(view, position + 42);
    const entryEnd =
      position + 46 + nameLength + extraLength + entryCommentLength;
    if (
      entryEnd > endOffset ||
      diskStart !== 0 ||
      compressedSize === ZIP64_VALUE_32 ||
      uncompressedSize === ZIP64_VALUE_32 ||
      localOffset === ZIP64_VALUE_32
    ) {
      throw new ArtifactSafetyError("zip_unsupported_entry");
    }
    const nameBytes = bytes.slice(position + 46, position + 46 + nameLength);
    const name = decodeEntryName(nameBytes, flags);
    const isDirectory = name.endsWith("/");
    validateEntryPath(name, nameBytes, isDirectory, limits);
    if (inventory.has(name)) {
      throw new ArtifactSafetyError("zip_unsafe_path");
    }
    if (
      flags & ENCRYPTED_FLAGS ||
      (compression !== STORED && compression !== DEFLATED) ||
      !entryKindIsSupported(madeBy, externalAttributes, isDirectory)
    ) {
      throw new ArtifactSafetyError("zip_unsupported_entry");
    }
    if (
      isDirectory &&
      (compressedSize !== 0 || uncompressedSize !== 0 || compression !== STORED)
    ) {
      throw new ArtifactSafetyError("zip_unsupported_entry");
    }
    if (uncompressedSize > limits.maximumZipEntryBytes) {
      throw new ArtifactSafetyError("zip_entry_size_limit");
    }
    declaredTotal += uncompressedSize;
    if (
      !Number.isSafeInteger(declaredTotal) ||
      declaredTotal > limits.maximumZipTotalBytes
    ) {
      throw new ArtifactSafetyError("zip_total_size_limit");
    }
    if (
      compressionRatioExceeded(
        uncompressedSize,
        compressedSize,
        limits.maximumZipCompressionRatio,
      )
    ) {
      throw new ArtifactSafetyError("zip_compression_ratio_limit");
    }

    const local = inspectLocalEntry(
      view,
      bytes,
      localOffset,
      centralOffset,
      nameBytes,
      flags,
      compression,
      crc32,
      compressedSize,
      uncompressedSize,
    );
    ranges.push([localOffset, local.dataEnd]);
    inventory.set(name, {
      compressedSize,
      compression,
      crc32,
      isDirectory,
      name,
      nameBytes,
      uncompressedSize,
    });
    position = entryEnd;
  }
  if (position !== endOffset) {
    throw new ArtifactSafetyError("zip_malformed");
  }
  ranges.sort((left, right) => left[0] - right[0]);
  for (let index = 1; index < ranges.length; index += 1) {
    if (ranges[index][0] < ranges[index - 1][1]) {
      throw new ArtifactSafetyError("zip_malformed");
    }
  }
  return inventory;
}

export function unzipArchiveBounded(
  input,
  {
    collect = true,
    limits = DEFAULT_ARTIFACT_SAFETY_LIMITS,
    needles = [],
    startedAt = Date.now(),
  } = {},
) {
  const bytes = asBytes(input);
  const inventory = inspectZipArchive(bytes, limits, startedAt);
  const matcher = new BytePatternMatcher(needles);
  for (const entry of inventory.values()) {
    if (matcher.contains(entry.nameBytes)) {
      throw new ArtifactSafetyError("private_entry_name");
    }
  }

  const output = Object.create(null);
  const seen = new Set();
  let actualTotal = 0;
  const unzip = new Unzip((file) => {
    ensureScanTime(startedAt, limits);
    const expected = inventory.get(file.name);
    if (
      !expected ||
      seen.has(file.name) ||
      file.compression !== expected.compression ||
      (Number.isInteger(file.size) && file.size !== expected.compressedSize) ||
      (Number.isInteger(file.originalSize) &&
        file.originalSize !== expected.uncompressedSize)
    ) {
      throw new ArtifactSafetyError("zip_malformed");
    }
    seen.add(file.name);
    const scanner = new StreamingPatternScanner(matcher);
    const chunks = [];
    let actual = 0;
    let crc = 0xffffffff;
    file.ondata = (error, data, final) => {
      if (error) {
        if (error instanceof ArtifactSafetyError) throw error;
        throw new ArtifactSafetyError("zip_malformed");
      }
      ensureScanTime(startedAt, limits);
      actual += data.length;
      actualTotal += data.length;
      if (actual > limits.maximumZipEntryBytes) {
        throw new ArtifactSafetyError("zip_entry_size_limit");
      }
      if (actualTotal > limits.maximumZipTotalBytes) {
        throw new ArtifactSafetyError("zip_total_size_limit");
      }
      if (
        compressionRatioExceeded(
          actual,
          expected.compressedSize,
          limits.maximumZipCompressionRatio,
        )
      ) {
        throw new ArtifactSafetyError("zip_compression_ratio_limit");
      }
      scanner.push(data);
      crc = updateCrc32(crc, data);
      if (collect && data.length !== 0) chunks.push(Uint8Array.from(data));
      if (final) {
        if (
          actual !== expected.uncompressedSize ||
          (crc ^ 0xffffffff) >>> 0 !== expected.crc32
        ) {
          throw new ArtifactSafetyError("zip_declared_size_mismatch");
        }
        if (collect) output[file.name] = concatenate(chunks, actual);
      }
    };
    file.start();
  });
  unzip.register(UnzipInflate);
  for (
    let offset = 0;
    offset < bytes.length;
    offset += ARTIFACT_SCAN_BUFFER_BYTES
  ) {
    ensureScanTime(startedAt, limits);
    const end = Math.min(offset + ARTIFACT_SCAN_BUFFER_BYTES, bytes.length);
    unzip.push(bytes.subarray(offset, end), end === bytes.length);
  }
  if (bytes.length === 0 || seen.size !== inventory.size) {
    throw new ArtifactSafetyError("zip_malformed");
  }
  return output;
}

export class BytePatternMatcher {
  constructor(needles) {
    this.nodes = [newMatcherNode()];
    this.maximumPatternBytes = 0;
    for (const value of needles) {
      const needle = asBytes(value);
      if (needle.length === 0) continue;
      this.maximumPatternBytes = Math.max(
        this.maximumPatternBytes,
        needle.length,
      );
      let state = 0;
      for (const byte of needle) {
        let next = this.nodes[state].edges.get(byte);
        if (next === undefined) {
          next = this.nodes.length;
          this.nodes[state].edges.set(byte, next);
          this.nodes.push(newMatcherNode());
        }
        state = next;
      }
      if (!this.nodes[state].outputs.includes(needle.length)) {
        this.nodes[state].outputs.push(needle.length);
      }
    }

    const queue = [];
    for (const child of this.nodes[0].edges.values()) queue.push(child);
    for (let head = 0; head < queue.length; head += 1) {
      const state = queue[head];
      for (const [byte, target] of this.nodes[state].edges) {
        queue.push(target);
        let fallback = this.nodes[state].failure;
        while (fallback !== 0 && !this.nodes[fallback].edges.has(byte)) {
          fallback = this.nodes[fallback].failure;
        }
        const linked = this.nodes[fallback].edges.get(byte);
        this.nodes[target].failure = linked === undefined ? 0 : linked;
        this.nodes[target].outputs.push(
          ...this.nodes[this.nodes[target].failure].outputs,
        );
      }
    }
    for (const node of this.nodes) {
      node.maximumOutputBytes = Math.max(0, ...node.outputs);
    }
  }

  advance(state, byte) {
    let current = state;
    while (current !== 0 && !this.nodes[current].edges.has(byte)) {
      current = this.nodes[current].failure;
    }
    return this.nodes[current].edges.get(byte) ?? 0;
  }

  contains(bytes) {
    let state = 0;
    for (const byte of bytes) {
      state = this.advance(state, byte);
      if (this.nodes[state].maximumOutputBytes !== 0) return true;
    }
    return false;
  }

  redact(bytes) {
    const output = Uint8Array.from(bytes);
    let state = 0;
    for (let index = 0; index < bytes.length; index += 1) {
      state = this.advance(state, bytes[index]);
      const patternBytes = this.nodes[state].maximumOutputBytes;
      if (patternBytes !== 0) {
        output.fill(42, index + 1 - patternBytes, index + 1);
      }
    }
    return output;
  }
}

export class StreamingPatternScanner {
  constructor(needlesOrMatcher) {
    this.matcher =
      needlesOrMatcher instanceof BytePatternMatcher
        ? needlesOrMatcher
        : new BytePatternMatcher(needlesOrMatcher);
    this.state = 0;
    this.bufferedBytes = 0;
  }

  push(chunk) {
    for (const byte of chunk) {
      this.state = this.matcher.advance(this.state, byte);
      if (this.matcher.nodes[this.state].maximumOutputBytes !== 0) {
        throw new ArtifactSafetyError("private_value");
      }
    }
  }
}

function inspectLocalEntry(
  view,
  bytes,
  offset,
  centralOffset,
  expectedName,
  expectedFlags,
  expectedCompression,
  expectedCrc32,
  expectedCompressedSize,
  expectedUncompressedSize,
) {
  if (readUint32(view, offset) !== LOCAL_FILE_ENTRY) {
    throw new ArtifactSafetyError("zip_malformed");
  }
  const flags = readUint16(view, offset + 6);
  const compression = readUint16(view, offset + 8);
  const crc32 = readUint32(view, offset + 14);
  const compressedSize = readUint32(view, offset + 18);
  const uncompressedSize = readUint32(view, offset + 22);
  const nameLength = readUint16(view, offset + 26);
  const extraLength = readUint16(view, offset + 28);
  const dataStart = offset + 30 + nameLength + extraLength;
  const dataEnd = dataStart + expectedCompressedSize;
  if (
    flags !== expectedFlags ||
    compression !== expectedCompression ||
    dataEnd > centralOffset ||
    !equalBytes(
      bytes.subarray(offset + 30, offset + 30 + nameLength),
      expectedName,
    )
  ) {
    throw new ArtifactSafetyError("zip_malformed");
  }
  if (
    !(flags & DATA_DESCRIPTOR_FLAG) &&
    (crc32 !== expectedCrc32 ||
      compressedSize !== expectedCompressedSize ||
      uncompressedSize !== expectedUncompressedSize)
  ) {
    throw new ArtifactSafetyError("zip_declared_size_mismatch");
  }
  return { dataEnd };
}

function findEndOfCentralDirectory(view) {
  if (view.byteLength < 22) throw new ArtifactSafetyError("zip_malformed");
  const minimum = Math.max(0, view.byteLength - 22 - 0xffff);
  for (let offset = view.byteLength - 22; offset >= minimum; offset -= 1) {
    if (readUint32(view, offset) === END_OF_CENTRAL_DIRECTORY) return offset;
  }
  throw new ArtifactSafetyError("zip_malformed");
}

function readUint16(view, offset) {
  if (offset < 0 || offset + 2 > view.byteLength) {
    throw new ArtifactSafetyError("zip_malformed");
  }
  return view.getUint16(offset, true);
}

function readUint32(view, offset) {
  if (offset < 0 || offset + 4 > view.byteLength) {
    throw new ArtifactSafetyError("zip_malformed");
  }
  return view.getUint32(offset, true);
}

function decodeEntryName(bytes, flags) {
  if (!(flags & UTF8_NAME_FLAG) && bytes.some((byte) => byte >= 0x80)) {
    throw new ArtifactSafetyError("zip_unsafe_path");
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw new ArtifactSafetyError("zip_unsafe_path");
  }
}

function validateEntryPath(name, bytes, isDirectory, limits) {
  if (
    bytes.length === 0 ||
    bytes.length > limits.maximumZipEntryNameBytes ||
    bytes.includes(0) ||
    name.startsWith("/") ||
    name.includes("\\") ||
    name.includes(":") ||
    name.endsWith("/") !== isDirectory
  ) {
    throw new ArtifactSafetyError("zip_unsafe_path");
  }
  const normalized = isDirectory ? name.slice(0, -1) : name;
  const components = normalized.split("/");
  if (
    components.length === 0 ||
    components.length > limits.maximumZipPathComponents ||
    components.some(
      (component) =>
        component.length === 0 ||
        component === "." ||
        component === ".." ||
        new TextEncoder().encode(component).length > 255,
    )
  ) {
    throw new ArtifactSafetyError("zip_unsafe_path");
  }
}

function entryKindIsSupported(madeBy, externalAttributes, isDirectory) {
  if (madeBy >>> 8 !== 3) return true;
  const kind = (externalAttributes >>> 16) & 0xf000;
  return kind === 0 || kind === (isDirectory ? 0x4000 : 0x8000);
}

function compressionRatioExceeded(uncompressed, compressed, maximumRatio) {
  return (
    uncompressed !== 0 &&
    (compressed === 0 || uncompressed > compressed * maximumRatio)
  );
}

function ensureScanTime(startedAt, limits) {
  if (Date.now() - startedAt >= limits.maximumDurationMilliseconds) {
    throw new ArtifactSafetyError("scan_time_limit");
  }
}

function asBytes(value) {
  if (value instanceof Uint8Array) return value;
  return new Uint8Array(value);
}

function equalBytes(left, right) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return false;
  }
  return true;
}

function newMatcherNode() {
  return { edges: new Map(), failure: 0, maximumOutputBytes: 0, outputs: [] };
}

function concatenate(chunks, length) {
  const output = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.length;
  }
  return output;
}

const CRC32_TABLE = Uint32Array.from({ length: 256 }, (_, value) => {
  let crc = value;
  for (let bit = 0; bit < 8; bit += 1) {
    crc = crc & 1 ? 0xedb88320 ^ (crc >>> 1) : crc >>> 1;
  }
  return crc >>> 0;
});

function updateCrc32(crc, bytes) {
  let value = crc >>> 0;
  for (const byte of bytes) {
    value = CRC32_TABLE[(value ^ byte) & 0xff] ^ (value >>> 8);
  }
  return value >>> 0;
}
