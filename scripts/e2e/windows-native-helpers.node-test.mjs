import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  sameWindowsPath,
  waitForWebDriverSession,
} from "./windows-native-helpers.mjs";

test("sameWindowsPath canonicalizes aliases before comparing", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "mendimaru-windows-path-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const workspace = join(root, "workspace");
  const alias = join(root, "workspace-alias");
  await mkdir(workspace);
  await symlink(
    workspace,
    alias,
    process.platform === "win32" ? "junction" : "dir",
  );

  assert.equal(await sameWindowsPath(workspace, alias), true);
});

test("sameWindowsPath rejects distinct existing workspaces", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "mendimaru-windows-path-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const first = join(root, "first");
  const second = join(root, "second");
  await Promise.all([mkdir(first), mkdir(second)]);

  assert.equal(await sameWindowsPath(first, second), false);
});

test("sameWindowsPath fails closed when either path does not exist", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "mendimaru-windows-path-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const existing = join(root, "existing");
  await mkdir(existing);

  await assert.rejects(
    sameWindowsPath(existing, join(root, "missing")),
    /ENOENT|cannot find|could not find/i,
  );
});

test("WebDriver readiness waits for the status endpoint and main window session", async () => {
  let statusAttempts = 0;
  let sessionAttempts = 0;
  const client = {
    async createSession() {
      sessionAttempts += 1;
      if (sessionAttempts === 1) {
        throw new Error("main window is not available yet");
      }
    },
  };

  await waitForWebDriverSession({
    client,
    statusUrl: "http://127.0.0.1:1/status",
    getEarlyExit: () => undefined,
    timeoutMs: 1_000,
    pollIntervalMs: 1,
    fetchImpl: async () => {
      statusAttempts += 1;
      return { ok: statusAttempts >= 2 };
    },
  });

  assert.equal(statusAttempts, 3);
  assert.equal(sessionAttempts, 2);
});

test("WebDriver readiness counts only session creation attempts", async () => {
  let attempts = 0;

  await waitForWebDriverSession({
    client: { createSession: async () => undefined },
    statusUrl: "http://127.0.0.1:1/status",
    getEarlyExit: () => undefined,
    timeoutMs: 1_000,
    pollIntervalMs: 1,
    fetchImpl: async () => ({ ok: true }),
    onSessionAttempt: () => {
      attempts += 1;
    },
  });

  assert.equal(attempts, 1);
});

test("WebDriver readiness reports an app exit immediately", async () => {
  let fetchCalled = false;
  const started = performance.now();

  await assert.rejects(
    waitForWebDriverSession({
      client: { createSession: async () => undefined },
      statusUrl: "http://127.0.0.1:1/status",
      getEarlyExit: () => ({ code: 1, signal: null }),
      timeoutMs: 10_000,
      pollIntervalMs: 1,
      fetchImpl: async () => {
        fetchCalled = true;
        return { ok: true };
      },
    }),
    /exited before WebDriver was ready.*"code":1/,
  );

  assert.equal(fetchCalled, false);
  assert.ok(performance.now() - started < 1_000);
});

test("WebDriver readiness preserves the last retryable session error", async () => {
  await assert.rejects(
    waitForWebDriverSession({
      client: {
        createSession: async () => {
          throw new Error("no main window");
        },
      },
      statusUrl: "http://127.0.0.1:1/status",
      getEarlyExit: () => undefined,
      timeoutMs: 20,
      pollIntervalMs: 1,
      fetchImpl: async () => ({ ok: true }),
    }),
    /Timed out.*no main window/,
  );
});
