import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { assertPreserved } from "./test-browser-winboat-live.mjs";

const execute = promisify(execFile);
const gate = fileURLToPath(
  new URL("test-browser-winboat-live.mjs", import.meta.url),
);

test("live gate rejects identity replacement even when readiness and counts stay unchanged", () => {
  const before = {
    container: {
      id: "first",
      state: "running",
      ports: { "8080/tcp": [{ HostPort: "8080" }] },
    },
    compose: "original-hash",
    studio: { processId: 42, startedAt: "first" },
    keeper: { pid: 100, startTicks: "10" },
    rdp: [{ pid: 200, parent: 100, startTicks: "20" }],
  };
  for (const change of [
    (after) => {
      after.container.id = "replacement";
    },
    (after) => {
      after.container.ports["8080/tcp"][0].HostPort = "8081";
    },
    (after) => {
      after.compose = "restored-hash";
    },
    (after) => {
      after.studio.startedAt = "reused-pid";
    },
    (after) => {
      after.keeper.startTicks = "reused-pid";
    },
    (after) => {
      after.rdp[0].startTicks = "reused-pid";
    },
  ]) {
    const after = structuredClone(before);
    change(after);
    assert.throws(() => assertPreserved(before, after));
  }
});

test(
  "live observer requires linked ownership and detects VM replacement",
  { skip: process.platform !== "linux", timeout: 30_000 },
  async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "mm148-"));
    const studioId = "studio-4242-638908128000000000";
    const socketDirectory = path.join(root, "cli-sessions");
    await fs.mkdir(socketDirectory, { mode: 0o700 });
    const socketPath = path.join(
      socketDirectory,
      `s-${createHash("sha256").update(studioId).digest("hex").slice(0, 32)}.sock`,
    );
    const sockets = new Set();
    const server = net.createServer((socket) => {
      sockets.add(socket);
      socket.on("close", () => sockets.delete(socket));
      socket.on("data", (data) => {
        assert.equal(data.toString(), "status\n");
        socket.end(
          `${JSON.stringify({
            ok: true,
            session: {
              schemaVersion: "5.0.0",
              sessionId: studioId,
              processId: 4242,
              startedAt: "2025-08-15T00:00:00Z",
              version: "11.12.3",
              state: "running",
              connection: "connected",
              projectName: "private-project-canary",
            },
          })}\n`,
        );
      });
    });
    await new Promise((resolve) => server.listen(socketPath, resolve));
    await fs.chmod(socketPath, 0o600);
    // Real /proc identity with a harmless process name; no RDP executable is run.
    const rdp = spawn(
      "python3",
      [
        "-c",
        "import ctypes,time; ctypes.CDLL(None).prctl(15,b'xfreerdp'); time.sleep(60)",
      ],
      { stdio: "ignore" },
    );
    try {
      for (let attempt = 0; attempt < 100; attempt++) {
        if (
          (await fs.readFile(`/proc/${rdp.pid}/comm`, "utf8")).trim() ===
          "xfreerdp"
        )
          break;
        await delay(10);
      }
      const compose = path.join(root, "compose.yml");
      await fs.writeFile(compose, "private-compose-canary");
      await fs.writeFile(
        path.join(root, "config.json"),
        JSON.stringify({
          containerRuntime: "docker",
          containerName: "fixture",
          composeFile: compose,
        }),
      );
      const binary = path.join(root, "mendimaru-fixture");
      await fs.writeFile(
        binary,
        `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const root = process.env.MENDIMARU_CACHE_DIR;
const browser = process.argv[2] === 'browser';
if (browser) fs.writeFileSync(path.join(root, 'browser-ran'), 'yes');
console.log(JSON.stringify({data: browser ? {outcome:'passed',passed:1,failed:0} : {
 state:'ready',mode:'studio-run-locally',httpReady:true,
 studioSessionId:process.env.GATE_FIXTURE_MODE === 'unlinked' ? null : '${studioId}'
}}));
`,
        { mode: 0o700 },
      );
      await fs.writeFile(
        path.join(root, "docker"),
        `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
if (process.argv[2] !== 'inspect') process.exit(1);
const changed = process.env.GATE_FIXTURE_MODE === 'replaced' && fs.existsSync(path.join(process.env.MENDIMARU_CACHE_DIR, 'browser-ran'));
console.log(JSON.stringify({id:changed?'replaced':'original',state:'running',ports:{'8080/tcp':[{HostIp:'127.0.0.1',HostPort:'8080'}]}}));
`,
        { mode: 0o700 },
      );
      for (const mode of ["unlinked", "preserved", "replaced"]) {
        await fs.rm(path.join(root, "browser-ran"), { force: true });
        const result = await execute(process.execPath, [gate], {
          timeout: 15_000,
          env: {
            ...process.env,
            PATH: `${root}:${process.env.PATH}`,
            GATE_FIXTURE_MODE: mode,
            MENDIMARU_CONFIG_DIR: root,
            MENDIMARU_CACHE_DIR: root,
            MENDIMARU_E2E_BINARY: binary,
            MENDIMARU_E2E_KEEPER_PID: String(process.pid),
            MENDIMARU_E2E_RUNTIME_SESSION_ID: `runtime_${"a".repeat(32)}`,
            MENDIMARU_E2E_BROWSER_SUITE: path.join(
              root,
              "fixture.browser.json",
            ),
          },
        }).then(
          (result) => ({ ...result, code: 0 }),
          (error) => error,
        );
        assert.equal(result.code, mode === "preserved" ? 0 : 1, result.stderr);
        assert(!`${result.stdout}${result.stderr}`.includes("private-"));
        if (mode === "unlinked") {
          await assert.rejects(fs.stat(path.join(root, "browser-ran")), {
            code: "ENOENT",
          });
        } else {
          const report = JSON.parse(result.stdout);
          assert.equal(
            report.outcome,
            mode === "preserved" ? "passed" : "failed",
          );
          assert(report.samples.length > 1);
          if (mode === "preserved") assert.equal(report.runs.length, 2);
        }
      }
      assert.equal(
        await fs.readFile(compose, "utf8"),
        "private-compose-canary",
      );
    } finally {
      rdp.kill();
      await new Promise((resolve) => rdp.once("exit", resolve));
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
      await fs.rm(root, { recursive: true, force: true });
    }
  },
);
