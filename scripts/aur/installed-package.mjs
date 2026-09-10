import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import readline from "node:readline";
import { setTimeout, clearTimeout } from "node:timers";

const mode = process.argv[2];
assert(["missing", "ready"].includes(mode));
assert.notEqual(
  process.getuid(),
  0,
  "exercise the installed package as a normal user",
);
for (const name of Object.keys(process.env)) {
  assert(
    !/^(MENDIMARU_|NODE_PATH$|NODE_OPTIONS$|PLAYWRIGHT_|APPDIR$)/.test(name),
    `unexpected override: ${name}`,
  );
}
assert.equal(process.cwd(), "/tmp");
assert.equal(existsSync("/build"), false);
assert.equal(existsSync("/smoke/node_modules"), false);
const inventory = JSON.parse(await readFile("/smoke/inventory.json", "utf8"));
const browserCache = path.join(os.homedir(), ".cache/ms-playwright");
const before = await cacheFiles();
const doctor = invoke(["browser", "doctor"]);
assert.equal(doctor.minimumNodeVersion, inventory.minimumNodeVersion);
assert.equal(doctor.nodeSupported, true);
assert.equal(doctor.playwrightVersion, inventory.modules["@playwright/test"]);
assert.equal(doctor.downloadPolicy, "explicit-only");
assert.equal(doctor.ready, mode === "ready");
assert.equal(doctor.chromium.installed, mode === "ready");
assert.equal(doctor.chromium.launchable, mode === "ready");

const server = spawn(process.execPath, ["/smoke/fixture-server.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
});
try {
  const port = await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("fixture startup timed out")),
      10_000,
    );
    const lines = readline.createInterface({ input: server.stdout });
    const finish = (error, value) => {
      clearTimeout(timer);
      lines.close();
      if (error) reject(error);
      else resolve(value);
    };
    server.once("error", (error) => finish(error));
    server.once("exit", (code) => finish(new Error(`fixture exited: ${code}`)));
    lines.once("line", (line) => {
      try {
        finish(null, JSON.parse(line).port);
      } catch (error) {
        finish(error);
      }
    });
  });
  const args = [
    "browser",
    "test",
    "--base-url",
    `http://127.0.0.1:${port}/`,
    "--suite-path",
    "/smoke/smoke.browser.json",
    "--fail-on-console-error",
    "--fail-on-network-failure",
  ];
  const env = {
    ...process.env,
    MENDIMARU_TEST_USERNAME: "package-smoke",
    MENDIMARU_TEST_PASSWORD: "package-smoke-private-canary",
  };
  if (mode === "missing") {
    assert.deepEqual(before, []);
    invoke(args, { env, error: "precondition_failed" });
    // A missing Node executable must remain a structured prerequisite error.
    const noNode = path.join(os.homedir(), "empty-path");
    await mkdir(noNode);
    invoke(["browser", "doctor"], {
      env: { ...process.env, PATH: noNode },
      error: "precondition_failed",
    });
    assert.deepEqual(await cacheFiles(), []);
    console.log(
      JSON.stringify({
        passed: true,
        doctor,
        missingBrowserRejected: true,
        missingNodeRejected: true,
      }),
    );
  } else {
    const result = invoke(args, { env });
    assert.equal(result.outcome, "passed");
    assert.equal(result.passed, 1);
    assert.equal(result.failed, 0);
    const artifacts = invoke([
      "browser",
      "artifacts",
      "--session-id",
      result.sessionId,
    ]);
    assert(
      artifacts.length > 0,
      "the installed artifact-safety and fflate modules must produce artifacts",
    );
    assert.deepEqual(
      await cacheFiles(),
      before,
      "doctor and test must not install browsers",
    );
    console.log(JSON.stringify({ passed: true, doctor, result, artifacts }));
  }
} finally {
  server.kill("SIGTERM");
}

function invoke(args, { env = process.env, error } = {}) {
  const result = spawnSync(
    "/usr/bin/mendimaru",
    [...args, "--timeout-seconds", "60", "--json"],
    {
      cwd: "/tmp",
      env,
      encoding: "utf8",
      timeout: 70_000,
      maxBuffer: 4 * 1024 * 1024,
    },
  );
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, error ? 1 : 0, result.stderr || result.stdout);
  const envelope = JSON.parse(error ? result.stderr : result.stdout);
  assert.equal(envelope.ok, !error);
  if (error) assert.equal(envelope.error.code, error);
  return envelope.data;
}

async function cacheFiles() {
  return existsSync(browserCache)
    ? (await readdir(browserCache, { recursive: true })).sort()
    : [];
}
