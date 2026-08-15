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
const mode = process.argv[2] ?? "--smoke";
assert(
  mode === "--smoke" || mode === "--lifecycle",
  "usage: test-winboat-e2e.mjs [--smoke|--lifecycle]",
);
const lifecycle = mode === "--lifecycle";
const version = process.env.MENDIMARU_E2E_VERSION ?? "";
if (lifecycle) {
  assert.equal(
    process.env.MENDIMARU_E2E_ALLOW_MUTATION,
    "1",
    "set MENDIMARU_E2E_ALLOW_MUTATION=1 for the destructive lifecycle gate",
  );
  assert.match(
    version,
    /^\d+\.\d+\.\d+(?:\.\d+)?(?:-[0-9A-Za-z.-]+)?$/,
    "set MENDIMARU_E2E_VERSION to an absent disposable exact version",
  );
}
const liveTest = lifecycle
  ? "platform::tests::live_e2e_linux_winboat_backend_lifecycle"
  : "winboat::sessions::tests::live_e2e_lists_sessions_and_rejects_an_ended_identity";
const innerMarker = "MENDIMARU_WINBOAT_E2E_XVFB";
const lifecycleTimeout = 75 * 60_000;

assert.equal(
  process.platform,
  "linux",
  "the live WinBoat RemoteApp E2E runs on Linux only",
);
if (lifecycle) validateWindowClassifier();

if (process.env[innerMarker] !== "1") {
  const processBaseline = await relevantProcessSnapshot();
  const status = await runInherited(
    "xvfb-run",
    [
      "--auto-servernum",
      "--server-args=-screen 0 1280x800x24",
      process.execPath,
      fileURLToPath(import.meta.url),
      mode,
    ],
    { ...process.env, [innerMarker]: "1" },
  );
  const leakedDesktopProcesses =
    await waitForRelevantProcessBaseline(processBaseline);
  assert.deepEqual(
    leakedDesktopProcesses,
    [],
    `the WinBoat gate leaked Xvfb, window-manager, or FreeRDP processes:\n${leakedDesktopProcesses.join("\n")}`,
  );
  process.exitCode = status;
} else {
  const windowManager = spawn("xfwm4", ["--compositor=off"], {
    cwd: repository,
    detached: true,
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
        detached: true,
        env: process.env,
        stdio: "inherit",
      },
    );
    const completed = childExit(test);
    const observed = {
      forbiddenWindows: new Set(),
      studioWindows: new Set(),
    };
    const deadline = Date.now() + lifecycleTimeout;
    while (test.exitCode === null && test.signalCode === null) {
      await recordWindows(observed);
      if (Date.now() >= deadline) {
        await terminate(test);
        assert.fail(
          `the live WinBoat ${lifecycle ? "lifecycle" : "smoke"} gate timed out`,
        );
      }
      await delay(25);
    }
    const status = await completed;
    for (let sample = 0; sample < 4; sample += 1) {
      await recordWindows(observed);
      await delay(50);
    }
    const leakedProcesses = await processGroupLines(test.pid);
    assert.equal(
      status,
      0,
      `the live WinBoat ${lifecycle ? "lifecycle" : "session smoke"} exited with ${status}`,
    );
    assert.deepEqual(
      [...observed.forbiddenWindows],
      [],
      `background RemoteApp operations exposed windows:\n${[
        ...observed.forbiddenWindows,
      ].join("\n")}`,
    );
    assert.deepEqual(
      leakedProcesses,
      [],
      `the WinBoat gate leaked processes from the cargo test group:\n${leakedProcesses.join("\n")}`,
    );
    if (lifecycle) {
      assert(
        observed.studioWindows.size > 0,
        `Studio Pro ${version} never exposed a real RemoteApp window`,
      );
      const remaining = (await windowLines()).filter((line) =>
        isExpectedStudioWindow(parseWindow(line)),
      );
      assert.deepEqual(
        remaining,
        [],
        `Studio Pro ${version} windows remained after the close/delete lifecycle:\n${remaining.join("\n")}`,
      );
      process.stdout.write(
        `WinBoat lifecycle E2E: ${version} absent → installed → window observed → stopped → uninstalled, with zero unexpected RemoteApp windows or leaked processes\n`,
      );
    } else {
      process.stdout.write(
        "WinBoat session smoke: authenticated queries passed with zero unexpected RemoteApp windows or leaked processes\n",
      );
    }
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

async function recordWindows(observed) {
  for (const line of await windowLines()) {
    const classification = classifyWindow(line);
    if (classification === "forbidden") {
      observed.forbiddenWindows.add(line.trim());
    } else if (classification === "studio") {
      observed.studioWindows.add(line.trim());
    }
  }
}

function classifyWindow(line) {
  const window = parseWindow(line);
  if (
    /(?:windows\s+power\s*shell|powershell|conhost|terminal)/i.test(
      window?.title ?? line,
    )
  ) {
    return "forbidden";
  }
  if (isExpectedStudioWindow(window)) return "studio";
  if (window?.className.startsWith("rail.mendimaru-")) return "forbidden";
  return "other";
}

function parseWindow(line) {
  const match = line.match(/^\s*(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s*(.*)$/);
  if (!match) return null;
  return {
    className: match[3].toLowerCase(),
    title: match[5].trim(),
  };
}

function isExpectedStudioWindow(window) {
  if (!lifecycle) return false;
  return (
    window?.className === expectedStudioWindowClass() &&
    (window.title.length === 0 ||
      window.title === "N/A" ||
      /mendix\s+studio\s+pro/i.test(window.title))
  );
}

function expectedStudioWindowClass() {
  const slug = `mendimaru-studio-pro-${version}`
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return `rail.${slug}`;
}

function validateWindowClassifier() {
  const studioClass = expectedStudioWindowClass();
  assert.equal(
    classifyWindow(`0x01 0 ${studioClass} N/A`),
    "studio",
    "a title-less FreeRDP Studio window must be recognized",
  );
  assert.equal(
    classifyWindow(`0x01 0 ${studioClass} N/A N/A`),
    "studio",
    "FreeRDP's N/A Studio title must be recognized",
  );
  assert.equal(
    classifyWindow(`0x01 0 ${studioClass} N/A Mendix Studio Pro ${version}`),
    "studio",
  );
  for (const title of ["Windows PowerShell", "앱 선택", "Terminal"]) {
    assert.equal(
      classifyWindow(`0x01 0 ${studioClass} N/A ${title}`),
      "forbidden",
      `${title} must never be accepted as Studio Pro`,
    );
  }
  assert.equal(
    classifyWindow(
      `0x01 0 RAIL.mendimaru-install-studio-pro-${version.replaceAll(".", "-")} N/A Mendix Setup`,
    ),
    "forbidden",
    "an installer window must never be accepted as Studio Pro",
  );
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

async function processGroupLines(processGroup) {
  const result = await capture("ps", [
    "-o",
    "pid=,ppid=,stat=,comm=,args=",
    "-g",
    String(processGroup),
  ]);
  assert(
    result.status === 0 || (result.status === 1 && result.stdout.length === 0),
    "ps could not inspect the cargo test process group",
  );
  return result.stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

async function relevantProcessSnapshot() {
  const result = await capture("ps", ["-eo", "pid=,comm=,args="]);
  assert.equal(result.status, 0, "ps could not inspect desktop processes");
  return new Map(
    result.stdout
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const match = line.match(/^(\d+)\s+(\S+)\s+(.*)$/);
        return match ? { line, pid: match[1], command: match[2] } : null;
      })
      .filter(
        (process) =>
          process &&
          (/^(?:Xvfb|xfwm4)$/i.test(process.command) ||
            /freerdp/i.test(process.command)),
      )
      .map((process) => [process.pid, process.line]),
  );
}

async function waitForRelevantProcessBaseline(baseline) {
  const deadline = Date.now() + 5_000;
  while (true) {
    const current = await relevantProcessSnapshot();
    const leaked = [...current]
      .filter(([pid]) => !baseline.has(pid))
      .map(([, line]) => line);
    if (leaked.length === 0) return leaked;
    if (Date.now() >= deadline) return leaked;
    await delay(100);
  }
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
  signalTree(child, "SIGTERM");
  await Promise.race([childExit(child), delay(3_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    signalTree(child, "SIGKILL");
    await childExit(child);
  }
}

function signalTree(child, signal) {
  if (!child.pid) return;
  try {
    process.kill(-child.pid, signal);
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}
