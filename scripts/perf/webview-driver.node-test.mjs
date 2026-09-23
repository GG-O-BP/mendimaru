import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";

import {
  SCRIPT_TIMEOUT_MS,
  WebDriverClient,
  scriptRequestTimeoutMs,
} from "./webview-driver.mjs";

// A deliberately small stand-in for the two WebDriver servers this harness
// talks to: tauri-driver in front of WebKitWebDriver on Linux, and the in-app
// bridge on Windows. Both accept POST /session/{id}/timeouts and both answer a
// script deadline with HTTP 500 and the W3C "script timeout" error code, which
// is exactly the contract issue #191 depends on.
async function startStubDriver({ supportsTimeouts = true } = {}) {
  const state = {
    declaredScriptTimeouts: [],
    scriptTimeoutMs: 30_000,
    requests: [],
  };
  const server = http.createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      const body = raw ? JSON.parse(raw) : {};
      state.requests.push({ method: request.method, url: request.url, body });
      const send = (status, payload) => {
        response.writeHead(status, { "content-type": "application/json" });
        response.end(JSON.stringify(payload));
      };
      if (request.method === "POST" && request.url === "/session") {
        send(200, { value: { sessionId: "stub-session" } });
        return;
      }
      if (request.url === "/session/stub-session/timeouts") {
        if (!supportsTimeouts) {
          send(404, {
            value: {
              error: "unknown command",
              message: "timeouts are not supported",
              stacktrace: "",
            },
          });
          return;
        }
        state.declaredScriptTimeouts.push(body.script);
        state.scriptTimeoutMs = body.script;
        send(200, { value: null });
        return;
      }
      if (request.url === "/session/stub-session/execute/async") {
        // `slow` scripts model a backend that outlives the script deadline.
        if (body.args?.[0] === "slow") {
          send(500, {
            value: {
              error: "script timeout",
              message: `script timed out after ${state.scriptTimeoutMs}ms`,
              stacktrace: "",
            },
          });
          return;
        }
        send(200, { value: { ok: true, value: { ready: true } } });
        return;
      }
      if (request.url === "/session/stub-session/execute/sync") {
        send(200, { value: "Projects" });
        return;
      }
      if (request.url === "/unparseable") {
        response.writeHead(200, { "content-type": "application/json" });
        response.end("not json at all");
        return;
      }
      if (request.url === "/hang") {
        // Never answered: models a driver that stops responding entirely.
        return;
      }
      if (request.method === "DELETE") {
        send(200, { value: null });
        return;
      }
      send(404, {
        value: { error: "unknown command", message: "unknown", stacktrace: "" },
      });
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return {
    state,
    client: new WebDriverClient(`http://127.0.0.1:${port}`),
    async close() {
      await new Promise((resolve) => {
        server.closeAllConnections?.();
        server.close(resolve);
      });
    },
  };
}

test("a script request deadline always outlives the declared script timeout", () => {
  assert.ok(
    scriptRequestTimeoutMs() > SCRIPT_TIMEOUT_MS,
    "the default script request deadline must exceed the declared script timeout",
  );
  assert.ok(scriptRequestTimeoutMs(750) > 750);
});

test("opening a session declares the script timeout instead of inheriting it", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());

  await driver.client.createLinuxSession("/tmp/app.AppImage");

  assert.deepEqual(driver.state.declaredScriptTimeouts, [SCRIPT_TIMEOUT_MS]);
  assert.equal(driver.client.scriptTimeoutMs, SCRIPT_TIMEOUT_MS);
});

test("the Windows session declares the same script timeout", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());

  await driver.client.createWindowsSession();

  assert.deepEqual(driver.state.declaredScriptTimeouts, [SCRIPT_TIMEOUT_MS]);
});

test("a server script deadline is reported as a script timeout, not a generic failure", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());
  await driver.client.createLinuxSession("/tmp/app.AppImage");
  await driver.client.declareScriptTimeout(750);

  const failure = await driver.client
    .invoke("slow", {}, scriptRequestTimeoutMs(750))
    .then(
      () => undefined,
      (error) => error,
    );

  assert.ok(failure, "a script deadline must reject");
  assert.equal(failure.name, "ScriptTimeoutError");
  assert.equal(
    failure.webdriverCommand.endpoint,
    "/session/stub-session/execute/async",
  );
  assert.equal(failure.webdriverCommand.webdriverError, "script timeout");
  assert.equal(failure.webdriverCommand.scriptTimeoutMs, 750);
  assert.ok(
    failure.webdriverCommand.requestTimeoutMs > 750,
    "the request deadline must have outlived the script deadline",
  );
});

test("a script request deadline shorter than the script timeout is refused", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());
  await driver.client.createLinuxSession("/tmp/app.AppImage");

  // This is the exact configuration that orphaned a running script in #191:
  // the fetch is abandoned while the server keeps executing, so the next
  // WebDriver command queues behind it and dies on the script deadline.
  assert.throws(
    () => driver.client.executeAsync("return 1;", [], 750),
    /must exceed the declared 30000 ms script timeout/,
  );
  assert.throws(
    () => driver.client.executeSync("return 1;", [], SCRIPT_TIMEOUT_MS),
    /must exceed the declared 30000 ms script timeout/,
  );
  await assert.rejects(
    driver.client.invoke("get_environment_status", {}, 750),
    /must exceed the declared 30000 ms script timeout/,
  );
});

test("a script request deadline that outlives a tightened script timeout is refused", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());
  await driver.client.createLinuxSession("/tmp/app.AppImage");
  await driver.client.declareScriptTimeout(750);

  // The inverse mismatch, and the one that actually reached CI: the caller
  // asks for the 35 s deadline that belongs to the session default while the
  // server is still enforcing the 750 ms probe deadline. Left unguarded the
  // server ends the script at 750 ms and the failure surfaces as an
  // unexplained script timeout in whichever sample happens to be slow.
  await assert.rejects(
    driver.client.invoke("get_environment_status"),
    /does not match the declared 750 ms script timeout/,
  );

  await driver.client.declareScriptTimeout(SCRIPT_TIMEOUT_MS);
  const value = await driver.client.invoke("get_environment_status");
  assert.deepEqual(value, { ready: true });
  assert.deepEqual(driver.state.declaredScriptTimeouts, [
    SCRIPT_TIMEOUT_MS,
    750,
    SCRIPT_TIMEOUT_MS,
  ]);
});

test("a driver without the timeouts endpoint keeps working on the inherited default", async (t) => {
  const driver = await startStubDriver({ supportsTimeouts: false });
  t.after(() => driver.close());

  await driver.client.createLinuxSession("/tmp/app.AppImage");

  assert.equal(driver.client.scriptTimeoutMs, undefined);
  // Without a declared deadline the guard cannot know what is safe, so it must
  // not block the legacy client-abort probe the harness falls back to.
  assert.equal(
    await driver.client.invoke("get_environment_status", {}, 750).then(
      (value) => value.ready,
      () => "rejected",
    ),
    true,
  );
});

test("an abandoned request records the command that never answered", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());

  const failure = await driver.client.request("POST", "/hang", {}, 200).then(
    () => undefined,
    (error) => error,
  );

  assert.ok(failure, "an unanswered request must reject");
  assert.equal(failure.webdriverCommand.outcome, "request-deadline");
  assert.equal(failure.webdriverCommand.endpoint, "/hang");
  assert.equal(driver.client.lastCommand.endpoint, "/hang");
});

test("an unparseable body names its endpoint instead of throwing a bare SyntaxError", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());

  const failure = await driver.client.request("POST", "/unparseable", {}).then(
    () => undefined,
    (error) => error,
  );

  assert.ok(failure);
  assert.equal(failure.name, "WebDriverError");
  assert.match(failure.message, /\/unparseable/);
  assert.equal(failure.webdriverCommand.outcome, "unparseable-response");
});

test("a successful command is recorded and the closed session drops its deadline", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());
  await driver.client.createLinuxSession("/tmp/app.AppImage");

  const value = await driver.client.invoke("get_environment_status");
  assert.deepEqual(value, { ready: true });
  assert.equal(driver.client.lastCommand.outcome, "ok");
  assert.equal(
    driver.client.lastCommand.endpoint,
    "/session/stub-session/execute/async",
  );

  await driver.client.deleteSession();
  assert.equal(driver.client.scriptTimeoutMs, undefined);
  assert.equal(driver.client.sessionId, undefined);
});

test("declaring a non-positive script timeout is rejected", async (t) => {
  const driver = await startStubDriver();
  t.after(() => driver.close());
  await driver.client.createLinuxSession("/tmp/app.AppImage");

  await assert.rejects(
    driver.client.declareScriptTimeout(0),
    /positive integer/,
  );
  await assert.rejects(
    driver.client.declareScriptTimeout(1.5),
    /positive integer/,
  );
});
