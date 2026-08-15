import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const liveTest =
  "winboat::sessions::tests::live_e2e_lists_sessions_and_rejects_an_ended_identity";
const innerMarker = "MENDIMARU_WINBOAT_E2E_XVFB";

assert.equal(
  process.platform,
  "linux",
  "the live WinBoat RemoteApp E2E runs on Linux only",
);

if (process.env[innerMarker] !== "1") {
  const status = await runInherited(
    "xvfb-run",
    [
      "--auto-servernum",
      "--server-args=-screen 0 1280x800x24",
      process.execPath,
      fileURLToPath(import.meta.url),
    ],
    { ...process.env, [innerMarker]: "1" },
  );
  process.exitCode = status;
} else {
  const windowManager = spawn("xfwm4", ["--compositor=off"], {
    cwd: repository,
    env: process.env,
    stdio: "ignore",
  });
  try {
    await waitForWindowManager(windowManager);
    const test = spawn(
      "cargo",
      [
        "test",
        "--manifest-path",
        "src-tauri/Cargo.toml",
        liveTest,
        "--",
        "--ignored",
        "--exact",
        "--nocapture",
        "--test-threads=1",
      ],
      {
        cwd: repository,
        env: process.env,
        stdio: "inherit",
      },
    );
    const completed = childExit(test);
    const visibleRemoteApps = new Set();
    while (test.exitCode === null && test.signalCode === null) {
      await recordVisibleRemoteApps(visibleRemoteApps);
      await delay(25);
    }
    const status = await completed;
    await recordVisibleRemoteApps(visibleRemoteApps);
    await delay(50);
    await recordVisibleRemoteApps(visibleRemoteApps);
    assert.equal(
      status,
      0,
      `the live WinBoat session E2E exited with ${status}`,
    );
    assert.deepEqual(
      [...visibleRemoteApps],
      [],
      `background RemoteApp operations exposed windows:\n${[
        ...visibleRemoteApps,
      ].join("\n")}`,
    );
    process.stdout.write(
      "WinBoat E2E: authenticated RemoteApp operations passed with zero visible PowerShell windows\n",
    );
  } finally {
    await terminate(windowManager);
  }
}

async function waitForWindowManager(manager) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    assert.equal(
      manager.exitCode,
      null,
      "xfwm4 exited before the virtual desktop became ready",
    );
    if ((await capture("wmctrl", ["-m"])).status === 0) return;
    await delay(100);
  }
  throw new Error("xfwm4 did not become ready on the Xvfb display");
}

async function windowLines() {
  const result = await capture("wmctrl", ["-l", "-x"]);
  assert(
    result.status === 0 || (result.status === 1 && result.stdout.length === 0),
    "wmctrl could not inspect the virtual desktop",
  );
  return result.stdout.split(/\r?\n/).filter(Boolean);
}

async function recordVisibleRemoteApps(visibleRemoteApps) {
  for (const line of await windowLines()) {
    if (
      /\sRAIL\.mendimaru-[^\s]*/i.test(line) ||
      /(?:windows\s+power\s*shell|powershell|conhost|terminal)/i.test(line)
    ) {
      visibleRemoteApps.add(line.trim());
    }
  }
}

function capture(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: repository,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.once("error", reject);
    child.once("exit", (status) => resolve({ status, stdout }));
  });
}

function runInherited(command, args, env) {
  const child = spawn(command, args, {
    cwd: repository,
    env,
    stdio: "inherit",
  });
  return childExit(child);
}

function childExit(child) {
  if (child.exitCode !== null) return Promise.resolve(child.exitCode);
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (status) => resolve(status ?? 1));
  });
}

async function terminate(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([childExit(child), delay(3_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await childExit(child);
  }
}
