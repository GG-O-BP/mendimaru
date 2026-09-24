// Read-only gate for an already-running keeper-linked Studio F5 runtime.
// Session preparation/teardown belongs to the disposable VM lifecycle workflow.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function assertPreserved(before, after) {
  for (const field of ["container", "compose", "studio", "keeper", "rdp"]) {
    assert.deepEqual(after[field], before[field], `${field} changed`);
  }
}

async function command(binary, args, timeout = 10_000) {
  // Never publish stderr: external commands may include private paths.
  try {
    return (await execute(binary, args, { timeout, maxBuffer: 1024 * 1024 }))
      .stdout;
  } catch {
    throw new Error("live gate command failed or exceeded its deadline");
  }
}

export async function processIdentity(pid) {
  const stat = await fs.readFile(`/proc/${pid}/stat`, "utf8");
  const fields = stat
    .slice(stat.lastIndexOf(")") + 2)
    .trim()
    .split(/\s+/);
  assert(!["Z", "X"].includes(fields[0]), "observed process exited");
  return {
    pid: Number(pid),
    parent: Number(fields[1]),
    startTicks: fields[19],
  };
}

export async function rdpProcesses() {
  const identities = [];
  for (const pid of await fs.readdir("/proc")) {
    if (!/^\d+$/.test(pid)) continue;
    try {
      const stat = await fs.stat(`/proc/${pid}`);
      if (stat.uid !== process.getuid()) continue;
      const comm = await fs.readFile(`/proc/${pid}/comm`, "utf8");
      if (/freerdp/i.test(comm)) identities.push(await processIdentity(pid));
    } catch (error) {
      if (error.code !== "ENOENT" && error.code !== "ESRCH") throw error;
    }
  }
  return identities.sort((a, b) => a.pid - b.pid);
}

export async function ownerStatus(cache, sessionId) {
  const directory = path.join(cache, "cli-sessions");
  const socketPath = path.join(
    directory,
    `s-${digest(sessionId).slice(0, 32)}.sock`,
  );
  const parent = await fs.lstat(directory);
  const entry = await fs.lstat(socketPath);
  assert(parent.isDirectory() && entry.isSocket(), "keeper socket unavailable");
  assert.equal(parent.uid, process.getuid(), "keeper directory owner changed");
  assert.equal(entry.uid, process.getuid(), "keeper socket owner changed");
  assert.equal(parent.mode & 0o777, 0o700, "keeper directory is not private");
  assert.equal(entry.mode & 0o777, 0o600, "keeper socket is not private");
  const response = await new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    const timer = setTimeout(
      () => finish(new Error("keeper response timeout")),
      2000,
    );
    let bytes = Buffer.alloc(0);
    let finished = false;
    function finish(error, value) {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      socket.destroy();
      if (error) reject(error);
      else resolve(value);
    }
    socket.on("connect", () => socket.write("status\n"));
    socket.on("error", () => finish(new Error("keeper response unavailable")));
    socket.on("end", () => finish(new Error("keeper response incomplete")));
    socket.on("data", (chunk) => {
      bytes = Buffer.concat([bytes, chunk]);
      if (bytes.length > 64 * 1024)
        return finish(new Error("keeper response oversized"));
      const end = bytes.indexOf(10);
      if (end < 0) return;
      try {
        finish(null, JSON.parse(bytes.subarray(0, end).toString("utf8")));
      } catch {
        finish(new Error("keeper response invalid"));
      }
    });
  });
  assert.equal(response.ok, true, "keeper observation failed");
  const session = response.session;
  assert.equal(session?.schemaVersion, "5.0.0");
  assert.equal(session.sessionId, sessionId);
  assert.equal(session.state, "running");
  assert.equal(session.connection, "connected");
  assert.match(session.version, /^\d+\.\d+\.\d+(?:\.\d+)?(?:-[\w.-]+)?$/);
  assert.equal(Number(sessionId.split("-")[1]), session.processId);
  assert(
    Number.isFinite(Date.parse(session.startedAt)),
    "missing Studio start identity",
  );
  // Exclude the project name and all host/guest paths from published evidence.
  return {
    sessionId,
    processId: session.processId,
    startedAt: session.startedAt,
    version: session.version,
    state: session.state,
    connection: session.connection,
  };
}

export async function runLiveGate() {
  assert.equal(process.platform, "linux", "live WinBoat gate requires Linux");
  function required(name) {
    assert(process.env[name], `set ${name}`);
    return process.env[name];
  }
  const binary = required("MENDIMARU_E2E_BINARY");
  const runtimeId = required("MENDIMARU_E2E_RUNTIME_SESSION_ID");
  const suite = required("MENDIMARU_E2E_BROWSER_SUITE");
  const keeperPid = required("MENDIMARU_E2E_KEEPER_PID");
  const configDirectory = required("MENDIMARU_CONFIG_DIR");
  const cache = required("MENDIMARU_CACHE_DIR");
  assert.match(runtimeId, /^runtime_[a-f0-9]{32}$/);
  assert.match(keeperPid, /^[1-9]\d*$/);
  for (const value of [binary, suite, configDirectory, cache])
    assert(path.isAbsolute(value));
  const config = JSON.parse(
    await fs.readFile(path.join(configDirectory, "config.json"), "utf8"),
  );
  assert(["docker", "podman"].includes(config.containerRuntime));
  const cli = async (...args) =>
    JSON.parse(
      await command(
        binary,
        [...args, "--json", "--timeout-seconds", "90"],
        100_000,
      ),
    );
  const status = (await cli("runtime", "status", "--session-id", runtimeId))
    .data;
  assert.equal(
    status?.state,
    "ready",
    "prepare Studio F5 and wait for HTTP readiness first",
  );
  assert.equal(status.mode, "studio-run-locally");
  assert.equal(status.httpReady, true);
  assert.match(status.studioSessionId ?? "", /^studio-\d+-\d+$/);
  const samples = [];
  async function snapshot() {
    const [containerText, compose, studio, keeper, rdp] = await Promise.all([
      command(config.containerRuntime, [
        "inspect",
        "--format",
        '{"id":{{json .Id}},"state":{{json .State.Status}},"ports":{{json .NetworkSettings.Ports}}}',
        config.containerName,
      ]),
      fs.readFile(config.composeFile),
      ownerStatus(cache, status.studioSessionId),
      processIdentity(keeperPid),
      rdpProcesses(),
    ]);
    const container = JSON.parse(containerText);
    assert.equal(container.state, "running");
    assert(
      rdp.some((client) => client.parent === Number(keeperPid)),
      "keeper has no live RDP child",
    );
    const sample = {
      at: new Date().toISOString(),
      container,
      compose: digest(compose),
      studio,
      keeper,
      rdp,
    };
    samples.push(sample);
    return sample;
  }
  const baseline = await snapshot();
  let done = false;
  let monitorError;
  const monitor = (async () => {
    while (!done) {
      await delay(500);
      try {
        assertPreserved(baseline, await snapshot());
      } catch (error) {
        monitorError = error;
        return;
      }
    }
  })();
  const runs = [];
  let failure;
  try {
    for (let repeat = 0; repeat < 2; repeat++) {
      const result = (
        await cli(
          "browser",
          "test",
          "--runtime-session-id",
          runtimeId,
          "--suite-path",
          suite,
          "--fail-on-console-error",
          "--fail-on-network-failure",
        )
      ).data;
      assert.equal(result?.outcome, "passed", "live browser suite failed");
      assert(
        result.passed > 0 && result.failed === 0,
        "live suite must execute assertions",
      );
      runs.push({ passed: result.passed, failed: result.failed });
      for (let read = 0; read < 5; read++) {
        const current = (
          await cli("runtime", "status", "--session-id", runtimeId)
        ).data;
        assert.equal(current?.state, "ready");
        assert.equal(current.studioSessionId, status.studioSessionId);
        assertPreserved(baseline, await snapshot());
      }
    }
    // Allow automatic keeper cleanup to run if an observation incorrectly ended Studio.
    await delay(3000);
    assertPreserved(baseline, await snapshot());
  } catch (error) {
    failure = error;
  } finally {
    done = true;
    await monitor;
  }
  const report = {
    issue: 148,
    outcome: failure || monitorError ? "failed" : "passed",
    runs,
    samples,
  };
  process.stdout.write(`${JSON.stringify(report)}\n`);
  if (failure || monitorError)
    throw new Error(
      "live browser/identity regression gate failed; inspect the safe report",
    );
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  runLiveGate().catch(() => {
    process.stderr.write(
      "Live WinBoat browser gate failed; verify the documented prerequisites and private local diagnostics.\n",
    );
    process.exitCode = 1;
  });
}
