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
  "schemas/browser-suite.schema.json",
  "schemas/browser.schema.json",
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
    schemaVersion: "3.0.0",
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
    schemaVersion: "3.0.0",
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
    schemaVersion: "3.0.0",
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
    schemaVersion: "3.0.0",
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
    schemaVersion: "3.0.0",
    command: "unknown",
    ok: false,
    platform: manifest.hostPlatform,
    backend: manifest.backend,
    sessionId: "session_unavailable",
    capabilitySnapshot: null,
    error: {
      schemaVersion: "3.0.0",
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
    schemaVersion: "3.0.0",
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
  schemaVersion: "3.0.0",
  artifactId: `artifact_${"34".repeat(16)}`,
  sessionId: runtimeSessionId,
  backend: manifest.backend,
  kind: "runtime-log",
  createdAt: snapshot.capturedAt,
  mediaType: "text/plain; charset=utf-8",
  location: `mendimaru-cache://artifact_${"34".repeat(16)}`,
};
const buildArtifact = (suffix, kind, mediaType) => ({
  schemaVersion: "3.0.0",
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
    schemaVersion: "3.0.0",
    sessionId: runtimeSessionId,
    backend: manifest.backend,
    mode: "portable",
    runtimeVersion: "11.12.2",
    state: "ready",
    httpReady: true,
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
    schemaVersion: "3.0.0",
    sessionId: runtimeSessionId,
    backend: "linux-winboat",
    mode: "studio-run-locally",
    state: "ready",
    httpReady: true,
    startedAt: snapshot.capturedAt,
    url: "http://127.0.0.1:49153",
    hostPort: 49153,
    guestPort: 8080,
    studioSessionId: "studio-4242-638908128000000000",
    studioState: "running",
    logArtifact: { ...runtimeLogArtifact, backend: "linux-winboat" },
  },
  "WinBoat Studio Run Locally ready status",
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

const browserSessionId = `session_${"90".repeat(16)}`;
const browserArtifact = {
  schemaVersion: "3.0.0",
  artifactId: `artifact_${"91".repeat(16)}`,
  sessionId: browserSessionId,
  backend: manifest.backend,
  kind: "browser-report",
  createdAt: snapshot.capturedAt,
  mediaType: "text/html; charset=utf-8",
  location: `mendimaru-cache://artifact_${"91".repeat(16)}`,
  sha256: "92".repeat(32),
  sizeBytes: 1024,
};
validate(
  `${schemas[8].$id}#/$defs/doctor`,
  {
    schemaVersion: "3.0.0",
    runnerVersion: "1.0.0",
    ready: true,
    nodeVersion: "22.22.2",
    minimumNodeVersion: "22.22.2",
    nodeSupported: true,
    playwrightVersion: "1.62.1",
    chromium: {
      installed: true,
      launchable: true,
      version: "151.0.7922.34",
    },
    downloadPolicy: "explicit-only",
  },
  "browser doctor",
);
validate(
  schemas[7].$id,
  {
    schemaVersion: "1.0.0",
    name: "Mendix fixture smoke",
    beforeEach: [
      { action: "goto", path: "/" },
      {
        action: "fill",
        locator: { by: "label", value: "Password" },
        valueFromEnv: "MENDIMARU_TEST_PASSWORD",
      },
    ],
    tests: [
      {
        name: "widget interaction",
        steps: [
          {
            action: "click",
            locator: { by: "mendixName", value: "SaveButton" },
          },
          {
            action: "expectText",
            locator: { by: "role", role: "status", name: "Result" },
            value: "Saved",
          },
        ],
      },
    ],
  },
  "browser suite",
);
reject(
  schemas[7].$id,
  {
    schemaVersion: "1.0.0",
    name: "Ambiguous credential source",
    tests: [
      {
        name: "invalid fill",
        steps: [
          {
            action: "fill",
            locator: { by: "label", value: "Password" },
            value: "must-not-be-accepted",
            valueFromEnv: "MENDIMARU_TEST_PASSWORD",
            sensitive: true,
          },
        ],
      },
    ],
  },
  "browser suite with two value sources",
);
validate(
  `${schemas[8].$id}#/$defs/summary`,
  {
    schemaVersion: "3.0.0",
    sessionId: browserSessionId,
    outcome: "passed",
    passed: 1,
    failed: 0,
    skipped: 0,
    startedAt: snapshot.capturedAt,
    finishedAt: snapshot.capturedAt,
    browserName: "chromium",
    browserVersion: "151.0.7922.34",
    playwrightVersion: "1.62.1",
    tests: [
      {
        name: "widget interaction",
        outcome: "passed",
        completedSteps: 2,
        totalSteps: 2,
      },
    ],
    artifacts: [browserArtifact],
  },
  "browser summary",
);
validate(
  `${schemas[8].$id}#/$defs/manifest`,
  {
    schemaVersion: "3.0.0",
    sessionId: browserSessionId,
    createdAt: snapshot.capturedAt,
    hostPlatform: "linux",
    studioPlatform: "windows",
    runtimePlatform: "windows",
    backend: "linux-winboat",
    runtimeMode: "studio-run-locally",
    studioVersion: "11.12.2",
    runtimeVersion: "11.12.2",
    browser: { name: "chromium", version: "151.0.7922.34" },
    playwrightVersion: "1.62.1",
    runnerVersion: "1.0.0",
    suite: { name: "Mendix fixture smoke", tests: 1 },
    policy: {
      navigationTimeoutMilliseconds: 30000,
      actionTimeoutMilliseconds: 10000,
      assertionTimeoutMilliseconds: 5000,
      failOnConsoleError: true,
      failOnNetworkFailure: true,
      recordVideo: false,
      recordHar: false,
      maxArtifactBytes: 134217728,
      retentionRuns: 20,
    },
    artifacts: [
      {
        file: "report.html",
        kind: "browser-report",
        mediaType: "text/html; charset=utf-8",
        sha256: "92".repeat(32),
        sizeBytes: 1024,
      },
    ],
  },
  "browser artifact manifest",
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
  assert(
    Array.isArray(manifest.runtimeModes) &&
      manifest.runtimeModes.includes("portable"),
    "Portable Runtime mode is not advertised",
  );
  if (manifest.backend === "linux-winboat") {
    assert(
      manifest.runtimeModes.includes("studio-run-locally"),
      "Linux WinBoat Run Locally mode is not advertised",
    );
    assert(!("runtimeMode" in manifest), "multi-mode manifest is ambiguous");
  } else {
    assert(manifest.runtimeMode === "portable", "Runtime mode is not portable");
  }
} else {
  assert(
    !("runtimePlatform" in manifest),
    "unsupported host has runtimePlatform",
  );
  assert(!("runtimeMode" in manifest), "unsupported host has runtimeMode");
  assert(!("runtimeModes" in manifest), "unsupported host has runtimeModes");
}
const browserCapabilities = manifest.capabilities.filter(({ id }) =>
  id.startsWith("browser."),
);
assert(
  browserCapabilities.every(({ status }) =>
    manifest.backend === "linux-winboat"
      ? status === "supported"
      : status === "unsupported",
  ),
  "Browser capability status does not match the Linux milestone",
);
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

function reject(id, value, label) {
  const validator = ajv.getSchema(id);
  assert(validator, `schema was not registered: ${id}`);
  if (validator(value)) {
    throw new Error(`${label} unexpectedly passed schema validation`);
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
