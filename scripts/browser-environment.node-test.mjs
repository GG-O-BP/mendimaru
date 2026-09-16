import process from "node:process";
import { readFile } from "node:fs/promises";
import Ajv from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { test } from "node:test";
import { EnvironmentMonitor, observerUrl } from "./browser-environment.mjs";

function report() {
  const snapshot = {
    observedAt: new Date().toISOString(),
    managedVm: "a".repeat(64),
    managementGeneration: "b".repeat(32),
    container: "c".repeat(64),
    containerRunning: true,
    compose: "d".repeat(64),
    publishedPorts: [],
    runtime: "e".repeat(64),
    studio: {
      processId: 42,
      startedAt: new Date().toISOString(),
      running: true,
    },
    build: "f".repeat(64),
    buildWatchGeneration: 0,
  };
  return {
    version: 1,
    preparationId: `preparation_${"a".repeat(32)}`,
    baseline: snapshot,
    latest: snapshot,
    observations: 1,
    events: [],
    comparable: true,
    missing: [],
    actor: "unknown",
    intervalMilliseconds: 1000,
  };
}

test("observer transport rejects remote, credential, fragment and redirected endpoints", () => {
  for (const value of [
    "http://example.com/",
    "http://user:secret@127.0.0.1:123/",
    "http://localhost:123/",
    `http://127.0.0.1:65536/observation_${"a".repeat(32)}`,
    `http://127.0.0.1:123/observation_${"a".repeat(32)}#x`,
  ])
    assert.throws(() => observerUrl(value));
});

test("observer interruption is sticky across recovery and a new preparation cannot overwrite evidence", async () => {
  let current = report();
  let unavailable = false;
  const server = createServer((_request, response) => {
    response.writeHead(unavailable ? 503 : 200, {
      "Content-Type": "application/json",
    });
    response.end(JSON.stringify(current));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const monitor = new EnvironmentMonitor(
    `http://127.0.0.1:${server.address().port}/observation_${"a".repeat(32)}`,
    `session_${"b".repeat(32)}`,
  );
  try {
    await monitor.start();
    assert.equal(monitor.report.comparable, true);
    unavailable = true;
    await monitor.sample();
    assert.equal(monitor.interrupted, true);
    assert.equal(
      monitor.report.events[0].classification,
      "observation-unavailable",
    );
    await assert.rejects(
      monitor.step(() => Promise.resolve()),
      /environment observation/,
    );
    unavailable = false;
    current = report();
    current.preparationId = `preparation_${"c".repeat(32)}`;
    await monitor.sample();
    assert.equal(monitor.report.comparable, false);
    assert.equal(monitor.report.preparationId, `preparation_${"a".repeat(32)}`);
    assert.equal(monitor.report.events.length, 1);
  } finally {
    await monitor.finish();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("missing endpoint preserves a bounded structured failure without its URL or exception", async () => {
  const monitor = new EnvironmentMonitor(
    `http://127.0.0.1:1/observation_${"a".repeat(32)}`,
    `session_${"b".repeat(32)}`,
  );
  await monitor.start();
  await monitor.finish();
  assert.equal(monitor.interrupted, true);
  const serialized = JSON.stringify(monitor.report);
  assert(!serialized.includes("http:"));
  assert(!serialized.includes("fetch failed"));
  assert(serialized.length < 4096);
});

test("the public environment schema rejects unbounded/private evidence and accepts complete reports", async () => {
  const source = JSON.parse(
    await readFile(
      new URL("../schemas/browser.schema.json", import.meta.url),
      "utf8",
    ),
  );
  const ajv = new Ajv({ strict: true });
  addFormats(ajv);
  const validate = ajv.compile({
    $defs: {
      environment: source.$defs.environment,
      environmentSnapshot: source.$defs.environmentSnapshot,
    },
    $ref: "#/$defs/environment",
  });
  assert.equal(validate(report()), true, JSON.stringify(validate.errors));
  const bad = report();
  bad.latest.privatePath = "/private-canary";
  assert.equal(validate(bad), false);
  delete bad.latest.privatePath;
  bad.events = Array(8).fill({
    classification: "build-changed",
    component: "build",
    observation: bad.latest,
  });
  assert.equal(validate(bad), false);
});

test(
  "a final environment change preserves the original browser assertion and matching manifest evidence",
  { skip: process.platform !== "linux", timeout: 30000 },
  async () => {
    const { spawn } = await import("node:child_process");
    const { mkdtemp, rm } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    const { fileURLToPath } = await import("node:url");
    const { setTimeout: delay } = await import("node:timers/promises");
    const directory = await mkdtemp(join(tmpdir(), "mm154-browser-"));
    const baseline = report();
    let changed = false;
    const server = createServer(async (request, response) => {
      if (request.url.startsWith("/observation_")) {
        const current = structuredClone(baseline);
        if (changed) {
          // Let the app assertion fail first; a later observation must augment it.
          await delay(500);
          current.latest.observedAt = new Date().toISOString();
          current.latest.container = "0".repeat(64);
          current.events = [
            {
              classification: "environment-changed",
              component: "container",
              observation: current.latest,
            },
          ];
          current.comparable = false;
          current.observations += 1;
        }
        response.end(JSON.stringify(current));
      } else {
        changed = true;
        response.writeHead(200, { "Content-Type": "text/html" });
        response.end("<!doctype html><html><body><p>ready</p></body></html>");
      }
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const baseUrl = `http://127.0.0.1:${server.address().port}/`;
    const request = {
      schemaVersion: "5.0.0",
      sessionId: `session_${"c".repeat(32)}`,
      baseUrl,
      outputDirectory: directory,
      environmentObserverUrl: `${baseUrl}observation_${"a".repeat(32)}`,
      runtimeContext: {
        hostPlatform: "linux",
        studioPlatform: "windows",
        backend: "linux-winboat",
        runtimeMode: "external-url",
      },
      policy: {
        navigationTimeoutMilliseconds: 5000,
        actionTimeoutMilliseconds: 1000,
        assertionTimeoutMilliseconds: 100,
        failOnConsoleError: false,
        failOnNetworkFailure: false,
        recordVideo: false,
        recordHar: false,
        maxArtifactBytes: 32 * 1024 * 1024,
        retentionRuns: 5,
      },
      suite: {
        schemaVersion: "1.0.0",
        name: "original failure",
        beforeEach: [{ action: "goto", path: "/" }],
        tests: [
          {
            name: "assertion",
            steps: [
              {
                action: "expectText",
                locator: { by: "text", value: "never-visible", exact: true },
                value: "never-visible",
              },
            ],
          },
        ],
      },
    };
    try {
      const child = spawn(
        process.execPath,
        [fileURLToPath(new URL("browser-runner.mjs", import.meta.url)), "run"],
        { stdio: ["pipe", "pipe", "pipe"] },
      );
      let stdout = "";
      let stderr = "";
      child.stdout.on("data", (chunk) => {
        stdout += chunk;
      });
      child.stderr.on("data", (chunk) => {
        stderr += chunk;
      });
      child.stdin.end(JSON.stringify(request));
      const code = await new Promise((resolve) => child.on("close", resolve));
      assert.equal(code, 0, stderr);
      const result = JSON.parse(stdout).data;
      assert.equal(result.outcome, "failed");
      assert.equal(result.failed, 1);
      assert.match(result.tests[0].failure, /never-visible/);
      assert.doesNotMatch(
        result.tests[0].failure,
        /environment observation interrupted/,
      );
      assert.equal(
        result.environment.events[0].classification,
        "environment-changed",
      );
      assert.equal(result.environment.comparable, false);
      for (const filename of ["summary.json", "artifact-manifest.json"]) {
        const artifact = JSON.parse(
          await readFile(join(directory, filename), "utf8"),
        );
        assert.deepEqual(artifact.environment, result.environment);
        assert(!JSON.stringify(artifact.environment).includes(baseUrl));
      }
    } finally {
      await new Promise((resolve) => server.close(resolve));
      await rm(directory, { recursive: true, force: true });
    }
  },
);
