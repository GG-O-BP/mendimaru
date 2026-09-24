import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

import {
  createProcessCpuTracker,
  trackProcessCpuSeconds,
  validateProcessTree,
} from "./performance-core.mjs";

const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";
const REQUEST_TIMEOUT_MS = 30_000;

// Issue #191. WebKitWebDriver and the in-app Windows WebDriver bridge both
// open a session with an *undeclared* script deadline that happens to be
// 30000 ms, which is why a stalled `execute/async` surfaced as an unexplained
// "script timed out after 30000ms" that matched no value in this repository.
// The harness now declares the deadline itself. The number is deliberately the
// same as the inherited one - this is a pinning change, not a relaxation - but
// it is now identical on both platforms, recorded in the report sampling
// policy, and the single value every script HTTP deadline is checked against.
export const SCRIPT_TIMEOUT_MS = 30_000;

// An `execute/async` HTTP deadline must stay strictly above the declared
// script deadline so the *server* is the component that ends a stalled script.
// A shorter HTTP deadline only aborts the local fetch: WebDriver has no
// command-cancel, so the script keeps running as an orphan and the next
// WebDriver command silently queues behind it until the script deadline
// expires. That is the mechanism behind the intermittent 30 s stall.
const SCRIPT_REQUEST_MARGIN_MS = 5_000;

export function scriptRequestTimeoutMs(scriptTimeoutMs = SCRIPT_TIMEOUT_MS) {
  return scriptTimeoutMs + SCRIPT_REQUEST_MARGIN_MS;
}

export async function createWebviewDriver({ application, env, root }) {
  if (process.platform === "linux") {
    return LinuxWebviewDriver.create({ application, env, root });
  }
  if (process.platform === "win32") {
    return WindowsWebviewDriver.create({ application, env, root });
  }
  throw new Error(
    `unsupported WebView performance platform: ${process.platform}`,
  );
}

class WebviewDriverBase {
  constructor({ application, env, root }) {
    this.application = path.resolve(application);
    this.env = env;
    this.root = root;
    this.client = undefined;
    this.output = "";
  }

  async waitForShell(timeoutMs = 20_000) {
    await waitFor(
      () =>
        this.client.executeSync(
          "return document.readyState === 'complete' && Boolean(document.querySelector('[data-testid=app-shell]'));",
        ),
      timeoutMs,
      "release application shell",
    );
  }

  async firstIpc() {
    const started = performance.now();
    const environment = await this.client.invoke("get_environment_status");
    assert.equal(environment.ready, true, "release backend must be ready");
    return rounded(performance.now() - started);
  }

  async screenshot(file) {
    const encoded = await this.client.screenshot();
    await fs.writeFile(file, Buffer.from(encoded, "base64"));
  }

  async close() {}
}

class LinuxWebviewDriver extends WebviewDriverBase {
  static async create(options) {
    const driver = new LinuxWebviewDriver(options);
    const driverPort = await availablePort();
    const nativePort = await availablePort();
    const tauriDriver = await findExecutable(
      process.env.TAURI_DRIVER_BINARY,
      path.join(os.homedir(), ".cargo", "bin", "tauri-driver"),
      "tauri-driver",
    );
    const webkitDriver = await findExecutable(
      process.env.WEBKIT_WEBDRIVER_BINARY,
      "/usr/bin/WebKitWebDriver",
      "WebKitWebDriver",
    );
    driver.driverProcess = startProcess(
      tauriDriver,
      [
        "--port",
        String(driverPort),
        "--native-port",
        String(nativePort),
        "--native-driver",
        webkitDriver,
      ],
      {
        env: options.env,
        label: "tauri-driver",
      },
    );
    driver.driverUrl = `http://127.0.0.1:${driverPort}`;
    await waitForHttp(
      `${driver.driverUrl}/status`,
      driver.driverProcess,
      20_000,
    );
    return driver;
  }

  async launch() {
    assert.equal(this.client, undefined, "a WebDriver session is already open");
    const started = performance.now();
    this.client = new WebDriverClient(this.driverUrl);
    const sessionStarted = performance.now();
    await this.client.createLinuxSession(this.application);
    const sessionFinished = performance.now();
    process.stdout.write(
      `release Linux WebDriver session created in ${rounded(
        sessionFinished - sessionStarted,
      )}ms\n`,
    );
    await this.waitForShell();
    const shellFinished = performance.now();
    this.applicationPid = await findLinuxApplication(
      this.application,
      this.root,
    );
    this.cpuTracker = createProcessCpuTracker();
    process.stdout.write(
      `release Linux launch stages: session=${rounded(
        sessionFinished - sessionStarted,
      )}ms shell=${rounded(shellFinished - sessionFinished)}ms process=${rounded(
        performance.now() - shellFinished,
      )}ms\n`,
    );
    return rounded(performance.now() - started);
  }

  async firstIpc() {
    const started = performance.now();
    const environment = await this.client.invokePolled(
      "get_environment_status",
    );
    assert.equal(environment.ready, true, "release backend must be ready");
    return rounded(performance.now() - started);
  }

  snapshot() {
    assert.ok(this.applicationPid, "the release application is not running");
    return linuxProcessSnapshot(this.applicationPid, this.cpuTracker);
  }

  async stop() {
    if (!this.client) return;
    await this.client.deleteSession().catch(() => undefined);
    this.client = undefined;
    await terminateLinuxApplications(this.application, this.root);
    this.applicationPid = undefined;
    this.cpuTracker = undefined;
  }

  webviewVersion() {
    return execFileSync("pkg-config", ["--modversion", "webkit2gtk-4.1"], {
      encoding: "utf8",
    }).trim();
  }

  osVersion() {
    return execFileSync("uname", ["-srv"], { encoding: "utf8" }).trim();
  }

  async close() {
    await this.stop();
    if (this.driverProcess) await terminateChild(this.driverProcess);
  }
}

class WindowsWebviewDriver extends WebviewDriverBase {
  static async create(options) {
    await fs.access(options.application);
    return new WindowsWebviewDriver(options);
  }

  async launch() {
    assert.equal(this.client, undefined, "a WebDriver session is already open");
    const port = await availablePort();
    const started = performance.now();
    this.appProcess = startProcess(this.application, [], {
      env: { ...this.env, TAURI_WEBDRIVER_PORT: String(port) },
      label: "mendimaru-release",
      windowsHide: true,
      detached: false,
    });
    this.client = new WebDriverClient(`http://127.0.0.1:${port}`);
    await waitForHttp(
      `http://127.0.0.1:${port}/status`,
      this.appProcess,
      20_000,
    );
    await createWindowsSessionWithWindowRetry(this.client);
    await this.waitForShell();
    this.cpuTracker = createProcessCpuTracker();
    return rounded(performance.now() - started);
  }

  snapshot() {
    assert.ok(this.appProcess?.pid, "the release application is not running");
    return windowsProcessSnapshot(this.appProcess.pid, this.cpuTracker);
  }

  async stop() {
    const applicationPid = this.appProcess?.pid;
    if (applicationPid) {
      // Kill the tree while its root still exists. Deleting the embedded
      // WebDriver session first can let the root exit before taskkill discovers
      // WebView2 descendants, leaving the user-data lock held briefly.
      terminateWindowsTree(applicationPid);
    } else if (this.client) {
      await this.client.deleteSession().catch(() => undefined);
    }
    this.client = undefined;
    await waitFor(
      () => !applicationPid || !processExists(applicationPid),
      10_000,
      "Windows release application exit",
    ).catch(() => undefined);
    this.appProcess = undefined;
    this.cpuTracker = undefined;
  }

  webviewVersion() {
    assert.ok(this.appProcess?.pid, "WebView version requires a running app");
    const script = `
      $records = @(Get-CimInstance Win32_Process -ErrorAction Stop)
      $ids = [Collections.Generic.HashSet[int]]::new()
      [void]$ids.Add(${this.appProcess.pid})
      do {
        $added = $false
        foreach ($record in $records) {
          if ($ids.Contains([int]$record.ParentProcessId) -and $ids.Add([int]$record.ProcessId)) {
            $added = $true
          }
        }
      } while ($added)
      $versions = @(
        $records |
          Where-Object { $ids.Contains([int]$_.ProcessId) -and $_.Name -ieq 'msedgewebview2.exe' } |
          ForEach-Object { (Get-Item -LiteralPath $_.ExecutablePath).VersionInfo.ProductVersion } |
          Sort-Object -Unique
      )
      if ($versions.Count -ne 1) { throw "Expected one WebView2 version; found $($versions -join ',')." }
      $versions[0]
    `;
    return powershell(script);
  }

  osVersion() {
    return powershell(
      "[Environment]::OSVersion.VersionString + ' ' + (Get-CimInstance Win32_OperatingSystem).Caption",
    );
  }

  async close() {
    await this.stop();
  }
}

export class WebDriverClient {
  constructor(baseUrl) {
    this.baseUrl = baseUrl;
    this.sessionId = undefined;
    // Declared script deadline for this session, or undefined while the
    // session has not declared one. Every script request deadline is checked
    // against it so the orphan configuration cannot be reintroduced silently.
    this.scriptTimeoutMs = undefined;
    // Last WebDriver command this client started. Retained even after it
    // fails so a harness failure can name the exact stalled endpoint.
    this.lastCommand = undefined;
    this.invocationSequence = 0;
  }

  async request(method, endpoint, body, timeoutMs = REQUEST_TIMEOUT_MS) {
    const started = performance.now();
    const command = {
      method,
      endpoint,
      requestTimeoutMs: timeoutMs,
      scriptTimeoutMs: this.scriptTimeoutMs,
      startedAt: new Date().toISOString(),
      outcome: "pending",
      elapsedMs: undefined,
    };
    this.lastCommand = command;
    let response;
    let text;
    try {
      response = await fetch(`${this.baseUrl}${endpoint}`, {
        method,
        headers:
          body === undefined
            ? undefined
            : { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeoutMs),
      });
      text = await response.text();
    } catch (error) {
      command.elapsedMs = rounded(performance.now() - started);
      command.outcome =
        error?.name === "TimeoutError" ? "request-deadline" : "transport-error";
      throw annotateWebDriverFailure(error, command);
    }
    command.elapsedMs = rounded(performance.now() - started);
    const envelope = parseWebDriverEnvelope(text, command);
    const webdriverError =
      envelope.value?.error &&
      envelope.value?.ok === undefined &&
      typeof envelope.value?.message === "string";
    if (!response.ok || webdriverError) {
      const code =
        typeof envelope.value?.error === "string"
          ? envelope.value.error
          : undefined;
      command.outcome = "webdriver-error";
      command.webdriverError = code ?? `http-${response.status}`;
      const failure = new Error(
        `WebDriver ${method} ${endpoint} failed (${response.status}) after ${command.elapsedMs} ms: ${text}`,
      );
      // A server-side script deadline is a distinct, expected outcome: the
      // deliberate timeout probe relies on it and the diagnosis in #191
      // depends on telling it apart from a client abort.
      failure.name =
        code === "script timeout" ? "ScriptTimeoutError" : "WebDriverError";
      throw annotateWebDriverFailure(failure, command);
    }
    command.outcome = "ok";
    return Object.hasOwn(envelope, "value") ? envelope.value : envelope;
  }

  // Declares the session script deadline through the W3C timeouts endpoint.
  // Returns false, without throwing, when a WebDriver implementation does not
  // support the endpoint; callers then keep the inherited behaviour rather
  // than failing a measurement for a diagnostic-only capability.
  async declareScriptTimeout(milliseconds = SCRIPT_TIMEOUT_MS) {
    assert.ok(
      Number.isInteger(milliseconds) && milliseconds > 0,
      "the declared script timeout must be a positive integer",
    );
    assert.ok(this.sessionId, "no WebDriver session is open");
    try {
      await this.request("POST", `/session/${this.sessionId}/timeouts`, {
        script: milliseconds,
      });
    } catch (error) {
      this.scriptTimeoutMs = undefined;
      process.stdout.write(
        `release WebDriver script timeout could not be declared (${
          error instanceof Error ? error.message : String(error)
        }); the inherited server default stays in effect\n`,
      );
      return false;
    }
    this.scriptTimeoutMs = milliseconds;
    return true;
  }

  async createLinuxSession(application) {
    const created = await this.request(
      "POST",
      "/session",
      {
        capabilities: {
          alwaysMatch: {
            browserName: "wry",
            "tauri:options": { application },
          },
        },
      },
      60_000,
    );
    this.sessionId = created.sessionId;
    assert.ok(this.sessionId, "tauri-driver did not return a session");
    await this.declareScriptTimeout();
  }

  async createWindowsSession() {
    const created = await this.request("POST", "/session", {
      capabilities: {
        alwaysMatch: {
          browserName: "tauri",
          "wdio:tauriServiceOptions": { windowLabel: "main" },
        },
      },
    });
    this.sessionId = created.sessionId;
    assert.ok(this.sessionId, "WebDriver did not return a session");
    await this.declareScriptTimeout();
  }

  async deleteSession() {
    if (!this.sessionId) return;
    await this.request("DELETE", `/session/${this.sessionId}`);
    this.sessionId = undefined;
    this.scriptTimeoutMs = undefined;
  }

  // Guards the orphan configuration described above. Without this a caller can
  // reintroduce #191 by passing a short HTTP deadline, and the symptom would
  // then appear one command later, inside unrelated code.
  assertScriptRequestDeadline(timeoutMs) {
    if (this.scriptTimeoutMs === undefined) return;
    if (timeoutMs <= this.scriptTimeoutMs) {
      throw new Error(
        `the ${timeoutMs} ms script request deadline must exceed the declared ${this.scriptTimeoutMs} ms script timeout; a shorter request deadline abandons the script as an orphan that blocks the next WebDriver command`,
      );
    }
    // The opposite mismatch is just as dangerous and is what actually broke
    // the first revision of this change. A caller that passes a deadline
    // derived from some *other* script timeout believes a deadline is in
    // force that the server is not enforcing: the server ends the script at
    // its own, much shorter, declared value and the caller reports an
    // unexplained script timeout. Requiring the HTTP deadline to be derived
    // from the currently declared script deadline turns that from an
    // intermittent, sample-dependent failure into a deterministic one at the
    // first offending command.
    const expected = scriptRequestTimeoutMs(this.scriptTimeoutMs);
    if (timeoutMs !== expected) {
      throw new Error(
        `the ${timeoutMs} ms script request deadline does not match the declared ${this.scriptTimeoutMs} ms script timeout; script requests must use scriptRequestTimeoutMs(${this.scriptTimeoutMs}) === ${expected} ms so the server deadline the caller relies on is the one actually in force`,
      );
    }
  }

  executeSync(script, args = [], timeoutMs = scriptRequestTimeoutMs()) {
    this.assertScriptRequestDeadline(timeoutMs);
    return this.request(
      "POST",
      `/session/${this.sessionId}/execute/sync`,
      { script, args },
      timeoutMs,
    );
  }

  executeAsync(script, args = [], timeoutMs = scriptRequestTimeoutMs()) {
    this.assertScriptRequestDeadline(timeoutMs);
    return this.request(
      "POST",
      `/session/${this.sessionId}/execute/async`,
      { script, args },
      timeoutMs,
    );
  }

  async invokeResult(
    command,
    payload = {},
    timeoutMs = scriptRequestTimeoutMs(),
  ) {
    return this.executeAsync(
      `
        const done = arguments[arguments.length - 1];
        window.__TAURI__.core.invoke(arguments[0], arguments[1]).then(
          (value) => done({ ok: true, value }),
          (error) => done({
            ok: false,
            error: typeof error === "string" ? error : JSON.stringify(error),
          }),
        );
      `,
      [command, payload],
      timeoutMs,
    );
  }

  async invoke(command, payload = {}, timeoutMs = scriptRequestTimeoutMs()) {
    const result = await this.invokeResult(command, payload, timeoutMs);
    if (!result.ok) throw new Error(`${command} failed: ${result.error}`);
    return result.value;
  }

  // #207: dispatch once, then observe the promise with synchronous commands.
  // This avoids tying the first IPC to WebKit's long-lived async-script
  // callback. It never retries the IPC or replaces a failed timing sample.
  async invokePolled(
    command,
    payload = {},
    { timeoutMs = SCRIPT_TIMEOUT_MS, pollMs = 25 } = {},
  ) {
    assert.ok(
      Number.isFinite(timeoutMs) &&
        timeoutMs > 0 &&
        timeoutMs <= SCRIPT_TIMEOUT_MS,
    );
    assert.ok(Number.isFinite(pollMs) && pollMs > 0);
    assert.ok(!this.pendingInvocation, "a polled IPC is already in flight");
    const token = `${this.sessionId}:${++this.invocationSequence}`;
    this.pendingInvocation = token;
    const deadline = performance.now() + timeoutMs;
    const execute = (script, args) =>
      this.request(
        "POST",
        `/session/${this.sessionId}/execute/sync`,
        { script, args },
        Math.max(1, Math.ceil(deadline - performance.now())),
      );
    try {
      await execute(
        `
        const [token, command, payload] = arguments;
        const state = { token, result: null };
        window.__mendimaruFirstIpc = state;
        const finish = result => {
          if (window.__mendimaruFirstIpc === state) state.result = result;
        };
        try {
          Promise.resolve(window.__TAURI__.core.invoke(command, payload)).then(
            value => finish({ ok: true, value }),
            error => finish({ ok: false, error: String(error) }),
          );
        } catch (error) {
          finish({ ok: false, error: String(error) });
        }
        return true;
      `,
        [token, command, payload],
      );
      while (performance.now() < deadline) {
        const state = await execute(
          `
          const state = window.__mendimaruFirstIpc;
          if (!state || state.token !== arguments[0]) return { missing: true };
          return { result: state.result };
        `,
          [token],
        );
        if (state?.missing)
          throw new Error(
            "first IPC observation was lost (session or page changed)",
          );
        if (state?.result) {
          if (!state.result.ok)
            throw new Error(`${command} failed: ${state.result.error}`);
          return state.result.value;
        }
        await delay(
          Math.min(pollMs, Math.max(0, deadline - performance.now())),
        );
      }
      const error = new Error(
        `first IPC ${command} did not settle within ${timeoutMs} ms`,
      );
      error.name = "IpcTimeoutError";
      throw error;
    } finally {
      this.pendingInvocation = undefined;
      // Invalidate the token even on failure. A late promise cannot publish
      // into the next measurement or leave a retained result in the page.
      await this.request(
        "POST",
        `/session/${this.sessionId}/execute/sync`,
        {
          script: `if (window.__mendimaruFirstIpc?.token === arguments[0]) delete window.__mendimaruFirstIpc; return true;`,
          args: [token],
        },
        1000,
      ).catch(() => undefined);
    }
  }

  async find(selector) {
    const found = await this.request(
      "POST",
      `/session/${this.sessionId}/element`,
      { using: "css selector", value: selector },
    );
    const id = found[ELEMENT_KEY];
    assert.ok(id, `WebDriver did not return an element for ${selector}`);
    return id;
  }

  async click(selector) {
    const id = await this.find(selector);
    await this.request(
      "POST",
      `/session/${this.sessionId}/element/${encodeURIComponent(id)}/click`,
      {},
    );
  }

  async text(selector) {
    const id = await this.find(selector);
    return this.request(
      "GET",
      `/session/${this.sessionId}/element/${encodeURIComponent(id)}/text`,
    );
  }

  screenshot() {
    return this.request("GET", `/session/${this.sessionId}/screenshot`);
  }
}

async function findLinuxApplication(application, expectedRoot) {
  const matches = await linuxApplicationProcesses(application, expectedRoot);
  const parentIds = new Set(matches.map((match) => match.parentPid));
  const leaves = matches.filter((match) => !parentIds.has(match.pid));
  assert.equal(
    leaves.length,
    1,
    `expected one release application leaf, found ${JSON.stringify(matches)}`,
  );
  return leaves[0].pid;
}

async function linuxApplicationProcesses(application, expectedRoot) {
  const expectedBinary = await fs.realpath(application);
  const acceptsBundledBinary = application.toLowerCase().endsWith(".appimage");
  const expectedEnvironment = `MENDIMARU_E2E_ROOT=${expectedRoot}`;
  const matches = [];
  for (const entry of await fs.readdir("/proc", { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^[1-9][0-9]*$/.test(entry.name)) continue;
    const pid = Number(entry.name);
    const binary = await fs.realpath(`/proc/${pid}/exe`).catch(() => undefined);
    if (
      binary !== expectedBinary &&
      !(acceptsBundledBinary && path.basename(binary ?? "") === "mendimaru")
    ) {
      continue;
    }
    const environment = await fs
      .readFile(`/proc/${pid}/environ`)
      .catch(() => undefined);
    if (
      environment?.toString("utf8").split("\0").includes(expectedEnvironment)
    ) {
      const stat = await fs.readFile(`/proc/${pid}/stat`, "utf8");
      const close = stat.lastIndexOf(")");
      const fields = stat
        .slice(close + 2)
        .trim()
        .split(/\s+/);
      matches.push({ pid, parentPid: Number(fields[1]) });
    }
  }
  return matches;
}

async function terminateLinuxApplications(application, expectedRoot) {
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    const matches = await linuxApplicationProcesses(application, expectedRoot);
    if (matches.length === 0) return;
    for (const { pid } of matches) {
      try {
        process.kill(-pid, signal);
      } catch {
        // A detached process may no longer be a process-group leader; its
        // direct PID is always killed below.
      }
      try {
        process.kill(pid, signal);
      } catch (error) {
        if (error.code !== "ESRCH") {
          throw error;
        }
      }
    }
    await delay(signal === "SIGTERM" ? 500 : 100);
  }
  const survivors = await linuxApplicationProcesses(application, expectedRoot);
  assert.equal(
    survivors.length,
    0,
    `release application processes survived cleanup: ${JSON.stringify(survivors)}`,
  );
}

async function createWindowsSessionWithWindowRetry(client) {
  let lastError;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      await client.createWindowsSession();
      return;
    } catch (error) {
      lastError = error;
      if (!String(error?.message).includes("no such window")) throw error;
      await delay(500);
    }
  }
  throw lastError;
}

async function linuxProcessSnapshot(rootPid, cpuTracker) {
  const records = [];
  for (const entry of await fs.readdir("/proc", { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^[1-9][0-9]*$/.test(entry.name)) continue;
    const pid = Number(entry.name);
    const stat = await fs
      .readFile(`/proc/${pid}/stat`, "utf8")
      .catch(() => undefined);
    if (!stat) continue;
    const close = stat.lastIndexOf(")");
    if (close < 0) continue;
    const fields = stat
      .slice(close + 2)
      .trim()
      .split(/\s+/);
    if (fields.length < 22) continue;
    records.push({
      pid,
      parentPid: Number(fields[1]),
      state: fields[0],
      cpuTicks: Number(fields[11]) + Number(fields[12]),
      startTicks: fields[19],
    });
  }
  const processIds = validateProcessTree(records, rootPid);
  let privateMemoryBytes = 0;
  let workingSetBytes = 0;
  let processCount = 0;
  for (const pid of processIds) {
    const record = records.find((candidate) => candidate.pid === pid);
    const memory = await linuxMemory(pid);
    // Linux keeps exited children as zombies until the host reaps them. They
    // have no scheduler, memory, or file resources and must not be counted as
    // a live WebView process leak.
    if (!record || record.state === "Z" || !memory) continue;
    privateMemoryBytes += memory.privateMemoryBytes;
    workingSetBytes += memory.workingSetBytes;
    processCount += 1;
  }
  assert.ok(processCount > 0, "could not sample the Linux process tree");
  const clockTicks = Number(
    execFileSync("getconf", ["CLK_TCK"], { encoding: "utf8" }).trim(),
  );
  const cpuSamples = processIds
    .map((pid) => records.find((record) => record.pid === pid))
    .filter(Boolean)
    .map((record) => ({
      identity: `${record.pid}:${record.startTicks}`,
      cpuSeconds: record.cpuTicks / clockTicks,
    }));
  return {
    processCount,
    privateMemoryBytes,
    workingSetBytes,
    cpuSeconds: trackProcessCpuSeconds(cpuTracker, cpuSamples),
  };
}

async function linuxMemory(pid) {
  const status = await fs
    .readFile(`/proc/${pid}/status`, "utf8")
    .catch(() => undefined);
  if (!status) return undefined;
  const workingSetBytes = procKilobytes(status, "VmRSS");
  const smaps = await fs
    .readFile(`/proc/${pid}/smaps_rollup`, "utf8")
    .catch(() => undefined);
  const privateMemoryBytes = smaps
    ? ["Private_Clean", "Private_Dirty", "Private_Hugetlb"].reduce(
        (total, name) => total + procKilobytes(smaps, name),
        0,
      )
    : workingSetBytes;
  return { privateMemoryBytes, workingSetBytes };
}

function procKilobytes(content, name) {
  const match = content.match(new RegExp(`^${name}:\\s+([0-9]+)\\s+kB$`, "m"));
  return match ? Number(match[1]) * 1024 : 0;
}

function windowsProcessSnapshot(rootPid, cpuTracker) {
  const script = `
    $records = @(Get-CimInstance Win32_Process -ErrorAction Stop)
    $ids = [Collections.Generic.HashSet[int]]::new()
    [void]$ids.Add(${rootPid})
    do {
      $added = $false
      foreach ($record in $records) {
        if ($ids.Contains([int]$record.ParentProcessId) -and $ids.Add([int]$record.ProcessId)) {
          $added = $true
        }
      }
    } while ($added)
    $selected = @($records | Where-Object { $ids.Contains([int]$_.ProcessId) })
    $processCount = 0
    $privateMemoryBytes = [long]0
    $workingSetBytes = [long]0
    $cpuSamples = @()
    foreach ($record in $selected) {
      $process = Get-Process -Id $record.ProcessId -ErrorAction SilentlyContinue
      if ($null -eq $process) { continue }
      $process.Refresh()
      $processCount += 1
      $privateMemoryBytes += $process.PrivateMemorySize64
      $workingSetBytes += $process.WorkingSet64
      $cpuSamples += [pscustomobject]@{
        identity = "$($record.ProcessId):$($record.CreationDate.ToUniversalTime().Ticks)"
        cpuSeconds = [double]$process.TotalProcessorTime.TotalSeconds
      }
    }
    [pscustomobject]@{
      records = @($selected | ForEach-Object {
        [pscustomobject]@{ pid = [int]$_.ProcessId; parentPid = [int]$_.ParentProcessId }
      })
      processCount = $processCount
      cpuSamples = $cpuSamples
      privateMemoryBytes = $privateMemoryBytes
      workingSetBytes = $workingSetBytes
    } | ConvertTo-Json -Depth 4 -Compress
  `;
  const snapshot = JSON.parse(powershell(script));
  const records = Array.isArray(snapshot.records)
    ? snapshot.records
    : [snapshot.records];
  validateProcessTree(records, rootPid);
  assert.ok(snapshot.processCount > 0, "could not sample Windows process tree");
  delete snapshot.records;
  const cpuSamples = Array.isArray(snapshot.cpuSamples)
    ? snapshot.cpuSamples
    : [snapshot.cpuSamples];
  snapshot.cpuSeconds = trackProcessCpuSeconds(cpuTracker, cpuSamples);
  delete snapshot.cpuSamples;
  return snapshot;
}

async function waitForHttp(url, child, timeoutMs) {
  await waitFor(
    async () => {
      assertRunning(child);
      try {
        return (await fetch(url, { signal: AbortSignal.timeout(2_000) })).ok;
      } catch {
        return false;
      }
    },
    timeoutMs,
    `${child.label} readiness`,
  );
}

async function waitFor(action, timeoutMs, description) {
  const deadline = performance.now() + timeoutMs;
  let lastError;
  while (performance.now() < deadline) {
    try {
      const value = await action();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  const failure = new Error(
    `timed out waiting for ${description}${lastError ? `: ${lastError.message}` : ""}`,
  );
  // Keep the underlying WebDriver command visible: a wait that expires because
  // every poll hit the same stalled endpoint must not lose that endpoint.
  if (lastError?.webdriverCommand) {
    failure.webdriverCommand = lastError.webdriverCommand;
  }
  failure.cause = lastError;
  throw failure;
}

function startProcess(command, args, options) {
  const child = spawn(command, args, {
    cwd: process.cwd(),
    detached: options.detached ?? process.platform !== "win32",
    env: options.env,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: options.windowsHide ?? true,
  });
  child.label = options.label;
  child.output = "";
  child.spawnError = undefined;
  child.once("error", (error) => {
    child.spawnError = error;
  });
  for (const stream of [child.stdout, child.stderr]) {
    stream?.on("data", (chunk) => {
      child.output = `${child.output}${chunk}`.slice(-128 * 1024);
    });
  }
  return child;
}

function assertRunning(child) {
  if (child.spawnError) throw child.spawnError;
  if (child.exitCode !== null || child.signalCode !== null) {
    throw new Error(
      `${child.label} exited with ${child.exitCode ?? child.signalCode}:\n${child.output}`,
    );
  }
}

async function terminateChild(child) {
  if (!child?.pid) return;
  if (process.platform === "win32") {
    terminateWindowsTree(child.pid);
    return;
  }
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
  await delay(500);
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function terminateWindowsTree(pid) {
  try {
    execFileSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true,
    });
  } catch {
    // The WebDriver session can close the app before taskkill observes it.
  }
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

async function availablePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert(address && typeof address !== "string");
  const { port } = address;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function findExecutable(...candidates) {
  for (const candidate of candidates.filter(Boolean)) {
    if (candidate.includes(path.sep)) {
      if (await isExecutable(candidate)) return path.resolve(candidate);
      continue;
    }
    for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
      const resolved = path.join(directory, candidate);
      if (await isExecutable(resolved)) return resolved;
    }
  }
  throw new Error(
    `missing executable: ${candidates.filter(Boolean).join(", ")}`,
  );
}

async function isExecutable(file) {
  try {
    await fs.access(file);
    return true;
  } catch {
    return false;
  }
}

function powershell(script) {
  return execFileSync(
    "powershell.exe",
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    { encoding: "utf8", windowsHide: true },
  ).trim();
}

function rounded(value) {
  return Math.round(value * 1000) / 1000;
}

// Parses a WebDriver response body without losing the command context. An
// unparseable body used to surface as a bare `SyntaxError` with no indication
// of which endpoint produced it.
function parseWebDriverEnvelope(text, command) {
  if (!text) return {};
  try {
    return JSON.parse(text);
  } catch (error) {
    command.outcome = "unparseable-response";
    const failure = new Error(
      `WebDriver ${command.method} ${command.endpoint} returned an unparseable body after ${command.elapsedMs} ms: ${text.slice(0, 500)}`,
    );
    failure.name = "WebDriverError";
    failure.cause = error;
    throw annotateWebDriverFailure(failure, command);
  }
}

// Attaches the failing command to the error so the harness can name the exact
// endpoint, deadline and elapsed time from logs and from the failure report,
// instead of only reporting the stack frame that happened to await it.
function annotateWebDriverFailure(error, command) {
  if (error && typeof error === "object") {
    try {
      error.webdriverCommand = command;
    } catch {
      // Frozen or exotic errors keep their original shape; the stdout line
      // below still records the command.
    }
  }
  // Only stall-shaped outcomes are logged here. Ordinary WebDriver errors are
  // expected inside polling loops such as `waitFor`, which retries every
  // 100 ms, so logging those would bury the signal this issue needs.
  if (stallShapedOutcomes.has(command.outcome) || isScriptTimeout(error)) {
    process.stdout.write(
      `release WebDriver command stalled: ${command.method} ${command.endpoint} outcome=${command.outcome} elapsed=${command.elapsedMs}ms requestDeadline=${command.requestTimeoutMs}ms scriptDeadline=${command.scriptTimeoutMs ?? "inherited"}\n`,
    );
  }
  return error;
}

const stallShapedOutcomes = new Set(["request-deadline", "transport-error"]);

export function isScriptTimeout(error) {
  return error?.name === "ScriptTimeoutError";
}

export function webdriverCommandOf(error) {
  return error?.webdriverCommand;
}
