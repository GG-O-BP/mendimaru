import fs from "node:fs";
import process from "node:process";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const schemaFiles = [
  "schemas/capabilities.schema.json",
  "schemas/backend-error.schema.json",
  "schemas/session.schema.json",
  "schemas/artifact.schema.json",
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

const snapshot = response.data;
const manifest = snapshot.manifest;
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

const expectedIds = new Set([
  "studio.detect",
  "studio.install",
  "studio.uninstall",
  "studio.start",
  "studio.status",
  "studio.stop",
  "runtime.build",
  "runtime.start",
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
