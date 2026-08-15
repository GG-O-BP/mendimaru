import fs from "node:fs";
import process from "node:process";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const schemaFiles = [
  "schemas/capabilities.schema.json",
  "schemas/backend-error.schema.json",
  "schemas/session.schema.json",
  "schemas/artifact.schema.json",
  "schemas/cli-response.schema.json",
  "schemas/cli-event.schema.json",
  "schemas/runtime.schema.json",
];
const schemas = schemaFiles.map((path) =>
  JSON.parse(fs.readFileSync(path, "utf8")),
);
const ajv = new Ajv2020({ allErrors: true, strict: true });
addFormats(ajv);
for (const schema of schemas) ajv.addSchema(schema);

let input = "";
process.stdin.setEncoding("utf8");
for await (const chunk of process.stdin) input += chunk;
const response = parseJson(input, "capabilities stdout");
validate(schemas[0].$id, response, "capabilities response");
validate(schemas[4].$id, response, "CLI response envelope");

const snapshot = response.data;
const manifest = snapshot.manifest;
assert(
  response.capabilitySnapshot.snapshotId === snapshot.snapshotId,
  "capability data and envelope snapshot differ",
);
assert(
  response.platform === manifest.hostPlatform,
  "CLI platform differs from snapshot",
);
assert(
  response.backend === manifest.backend,
  "CLI backend differs from snapshot",
);
const sessionId = `session_${"ab".repeat(16)}`;
validate(
  schemas[2].$id,
  {
    schemaVersion: "1.0.0",
    sessionId,
    createdAt: snapshot.capturedAt,
    state: "created",
    capabilitySnapshot: snapshot,
  },
  "session descriptor",
);
validate(
  schemas[3].$id,
  {
    schemaVersion: "1.0.0",
    artifactId: `artifact_${"cd".repeat(16)}`,
    sessionId,
    backend: manifest.backend,
    kind: "diagnostic",
    createdAt: snapshot.capturedAt,
  },
  "artifact descriptor",
);
validate(
  schemas[1].$id,
  {
    schemaVersion: "1.0.0",
    code: "unsupported_capability",
    message: "runtime.url is not implemented by this backend",
    backend: manifest.backend,
    capability: "runtime.url",
    reason: {
      code: "unsupported_capability",
      message: "runtime.url is not implemented by this backend",
    },
    retryable: false,
  },
  "backend error",
);
const parseErrorEnvelope = {
  ...response,
  command: "studio",
  ok: false,
  error: {
    schemaVersion: "1.0.0",
    code: "invalid_request",
    message: "the command request is invalid",
    backend: manifest.backend,
    retryable: false,
  },
};
delete parseErrorEnvelope.data;
validate(schemas[4].$id, parseErrorEnvelope, "CLI parse-error envelope");
validate(
  schemas[4].$id,
  {
    schemaVersion: "1.0.0",
    command: "unknown",
    ok: false,
    platform: manifest.hostPlatform,
    backend: manifest.backend,
    sessionId: "session_unavailable",
    capabilitySnapshot: null,
    error: {
      schemaVersion: "1.0.0",
      code: "operation_failed",
      message: "the command could not be completed",
      backend: manifest.backend,
      retryable: false,
    },
  },
  "CLI fatal bootstrap envelope",
);
validate(
  schemas[4].$id,
  {
    ...response,
    command: "studio.start",
    operationId: `launch-11.12.2-${"ef".repeat(16)}`,
    studioSessionId: "studio-4242-638908128000000000",
    data: { completed: true },
  },
  "CLI Studio start envelope",
);
validate(
  schemas[5].$id,
  {
    schemaVersion: "1.0.0",
    command: "studio.install",
    event: "progress",
    sessionId,
    progress: {
      version: "11.12.2",
      state: "downloading",
      downloadedBytes: 1024,
      totalBytes: 4096,
      percentage: 25,
      estimated: false,
      message: "Downloading Studio Pro",
    },
  },
  "CLI progress event",
);
const runtimeSessionId = `runtime_${"12".repeat(16)}`;
const runtimeBuildSessionId = `session_${"56".repeat(16)}`;
const runtimeLogArtifact = {
  schemaVersion: "1.0.0",
  artifactId: `artifact_${"34".repeat(16)}`,
  sessionId: runtimeSessionId,
  backend: manifest.backend,
  kind: "runtime-log",
  createdAt: snapshot.capturedAt,
  mediaType: "text/plain; charset=utf-8",
  location: `mendimaru-cache://artifact_${"34".repeat(16)}`,
};
const buildArtifact = (suffix, kind, mediaType) => ({
  schemaVersion: "1.0.0",
  artifactId: `artifact_${suffix.repeat(16)}`,
  sessionId: runtimeBuildSessionId,
  backend: manifest.backend,
  kind,
  createdAt: snapshot.capturedAt,
  mediaType,
  location: `mendimaru-cache://artifact_${suffix.repeat(16)}`,
  sha256: suffix.repeat(32),
  sizeBytes: 16,
});
validate(
  schemas[6].$id,
  {
    sessionId: runtimeBuildSessionId,
    packageArtifact: buildArtifact("67", "runtime-package", "application/zip"),
    consistencyArtifact: buildArtifact(
      "78",
      "consistency-report",
      "application/json",
    ),
    buildLogArtifact: buildArtifact(
      "89",
      "build-log",
      "text/plain; charset=utf-8",
    ),
    requiredVersion: "11.12.2",
    toolchainVersion: "11.12.2",
    cacheHit: false,
    capabilityBasis: "Mendix Portable Runtime documentation, 2026-06",
  },
  "Portable Runtime build result",
);
validate(
  schemas[6].$id,
  {
    schemaVersion: "1.0.0",
    sessionId: runtimeSessionId,
    mode: "portable",
    state: "ready",
    processId: 4242,
    startedAt: snapshot.capturedAt,
    url: "http://127.0.0.1:49152",
    logArtifact: runtimeLogArtifact,
  },
  "Portable Runtime ready status",
);
validate(
  schemas[6].$id,
  {
    sessionId: runtimeSessionId,
    entries: ["2026-08-15T00:00:00Z stdout runtime ready"],
    truncated: false,
  },
  "Portable Runtime log batch",
);

const expectedIds = new Set([
  "studio.detect",
  "studio.install",
  "studio.uninstall",
  "studio.start",
  "studio.status",
  "studio.stop",
  "runtime.build",
  "runtime.start",
  "runtime.status",
  "runtime.wait",
  "runtime.url",
  "runtime.stop",
  "runtime.logs",
  "ui.capabilities",
  "ui.tree",
  "ui.find",
  "ui.action",
  "ui.wait",
  "ui.screenshot",
  "browser.test",
  "browser.artifacts",
]);
const actualIds = new Set(manifest.capabilities.map(({ id }) => id));
assert(actualIds.size === expectedIds.size, "capability IDs must be unique");
for (const id of expectedIds) {
  assert(actualIds.has(id), `missing capability: ${id}`);
}
for (const capability of manifest.capabilities) {
  assert(
    capability.fallbackAllowed === false,
    `${capability.id} unexpectedly allows fallback`,
  );
  if (capability.status === "unsupported") {
    assert(capability.limitation, `${capability.id} has no limitation`);
    assert(
      capability.limitation.code === "unsupported_capability",
      `${capability.id} has the wrong unsupported code`,
    );
  }
}
const runtimeCapabilities = manifest.capabilities.filter(({ id }) =>
  id.startsWith("runtime."),
);
const portableHost = ["linux", "windows"].includes(manifest.hostPlatform);
assert(
  runtimeCapabilities.every(({ status }) =>
    portableHost ? status === "supported" : status === "unsupported",
  ),
  "Runtime capability status does not match the documented host matrix",
);
if (portableHost) {
  assert(
    manifest.runtimePlatform === manifest.hostPlatform,
    "Portable Runtime platform differs from the host",
  );
  assert(manifest.runtimeMode === "portable", "Runtime mode is not portable");
} else {
  assert(
    !("runtimePlatform" in manifest),
    "unsupported host has runtimePlatform",
  );
  assert(!("runtimeMode" in manifest), "unsupported host has runtimeMode");
}
for (const leaked of [
  "apiUrl",
  "rdpHost",
  "rdpPort",
  "composeFile",
  "windowsSharedDirectory",
]) {
  assert(!(leaked in manifest), `common manifest leaked ${leaked}`);
}

process.stdout.write(
  `backend contract schemas: valid (${manifest.backend}, ${manifest.capabilities.length} capabilities)\n`,
);

function parseJson(source, label) {
  try {
    return JSON.parse(source);
  } catch (error) {
    throw new Error(`${label} is not one JSON document: ${error.message}`, {
      cause: error,
    });
  }
}

function validate(id, value, label) {
  const validator = ajv.getSchema(id);
  assert(validator, `schema was not registered: ${id}`);
  if (!validator(value)) {
    throw new Error(
      `${label} failed schema validation:\n${ajv.errorsText(validator.errors, { separator: "\n" })}`,
    );
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
