import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:net";
import { cpus, platform, tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { performance } from "node:perf_hooks";
import process from "node:process";

const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";
const MARKER_NAME = ".mendimaru-e2e-root";
const MARKER_CONTENT = "mendimaru isolated native e2e\n";
const ROOT = resolve(import.meta.dirname, "..", "..");
const ARTIFACT_DIRECTORY = join(ROOT, "artifacts", "e2e");
const SKIP_LIVE_MARKETPLACE =
  process.env.MENDIMARU_E2E_SKIP_LIVE_MARKETPLACE === "1";
const SKIP_LIVE_STUDIO = process.env.MENDIMARU_E2E_SKIP_LIVE_STUDIO === "1";
const thresholds = {
  startupMs: numberEnvironment("MENDIMARU_E2E_MAX_STARTUP_MS", 120_000),
  environmentMs: numberEnvironment("MENDIMARU_E2E_MAX_ENVIRONMENT_MS", 5_000),
  installedMs: numberEnvironment("MENDIMARU_E2E_MAX_INSTALLED_MS", 20_000),
  projectsMs: numberEnvironment("MENDIMARU_E2E_MAX_PROJECTS_MS", 10_000),
  marketplaceMs: numberEnvironment("MENDIMARU_E2E_MAX_MARKETPLACE_MS", 120_000),
  studioLaunchMs: numberEnvironment(
    "MENDIMARU_E2E_MAX_STUDIO_LAUNCH_MS",
    120_000,
  ),
  navigationMs: numberEnvironment("MENDIMARU_E2E_MAX_NAVIGATION_MS", 3_000),
  privateMemoryBytes: numberEnvironment(
    "MENDIMARU_E2E_MAX_PRIVATE_MEMORY_BYTES",
    768 * 1024 * 1024,
  ),
  idleCpuPercent: numberEnvironment("MENDIMARU_E2E_MAX_IDLE_CPU_PERCENT", 10),
};

if (platform() !== "win32") {
  throw new Error("The native Windows E2E suite must run on Windows.");
}

let report;
const output = [];
let appProcess;
let client;
let failure;
let launchedStudioProcessId;

async function run() {
  await mkdir(ARTIFACT_DIRECTORY, { recursive: true });
  const isolatedRoot = await mkdtemp(join(tmpdir(), "mendimaru-e2e-"));
  const primaryWorkspace = join(isolatedRoot, "workspace");
  const secondaryWorkspace = join(isolatedRoot, "workspace-secondary");
  await writeFile(join(isolatedRoot, MARKER_NAME), MARKER_CONTENT, "utf8");
  await createProjectFixture(primaryWorkspace, "Orders", "11.12.2");
  await createProjectFixture(secondaryWorkspace, "Inventory", "10.24.9");

  report = {
    status: "running",
    startedAt: new Date().toISOString(),
    node: process.version,
    platform: `${platform()}-${process.arch}`,
    skipLiveMarketplace: SKIP_LIVE_MARKETPLACE,
    skipLiveStudio: SKIP_LIVE_STUDIO,
    thresholds,
    measurements: {},
    assertions: [],
  };

  try {
    const port = await availablePort();
    report.webdriverPort = port;
    const started = performance.now();
    const npmCli =
      process.env.npm_execpath ??
      join(
        dirname(process.execPath),
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      );
    await access(npmCli);
    appProcess = spawn(
      process.execPath,
      [
        npmCli,
        "run",
        "tauri",
        "--",
        "dev",
        "--no-watch",
        "--features",
        "e2e",
        "--config",
        "src-tauri/tauri.e2e.conf.json",
      ],
      {
        cwd: ROOT,
        env: {
          ...process.env,
          MENDIMARU_E2E_ROOT: isolatedRoot,
          TAURI_WEBDRIVER_PORT: String(port),
          VITE_MENDIMARU_E2E: "1",
        },
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
      },
    );
    captureOutput(appProcess.stdout, output, process.stdout);
    captureOutput(appProcess.stderr, output, process.stderr);
    let earlyExit;
    appProcess.once("exit", (code, signal) => {
      earlyExit = { code, signal };
    });

    await waitUntil(
      async () => {
        if (earlyExit) {
          throw new Error(
            `tauri dev exited before WebDriver was ready (${JSON.stringify(earlyExit)})`,
          );
        }
        const response = await fetch(`http://127.0.0.1:${port}/status`).catch(
          () => undefined,
        );
        return response?.ok;
      },
      thresholds.startupMs,
      "embedded WebDriver startup",
    );
    report.measurements.startupMs = rounded(performance.now() - started);
    assertThreshold("startupMs", report.measurements.startupMs);

    client = new WebDriverClient(port);
    await client.createSession();
    await waitUntil(
      () =>
        client.executeSync(
          "return document.readyState === 'complete' && Boolean(document.querySelector('[data-testid=app-shell]'));",
        ),
      30_000,
      "React application shell",
    );

    const csp = await client.executeSync(
      "return document.querySelector('meta[http-equiv=\"Content-Security-Policy\"]')?.content || '';",
    );
    report.cspMeta = csp;
    const dataScriptBlocked = await client.executeAsync(`
      const done = arguments[arguments.length - 1];
      Promise.resolve(window.__MENDIMARU_CSP_PROBE__).then(done, () => done(false));
    `);
    recordAssertion(
      dataScriptBlocked,
      "the live dev WebView blocks a data-script CSP probe",
    );
    recordAssertion(
      await client.executeSync(
        "return Boolean(window.__TAURI__?.core?.invoke);",
      ),
      "the isolated E2E build exposes the test-only Tauri invoke bridge",
    );

    const environmentTimed = await timed(() =>
      client.invoke("get_environment_status"),
    );
    report.measurements.environmentMs = environmentTimed.elapsedMs;
    assertThreshold("environmentMs", environmentTimed.elapsedMs);
    const environment = environmentTimed.value;
    recordAssertion(
      environment.platform.kind === "windows-native" &&
        environment.platform.requiresWinboat === false,
      "the actual backend reports native Windows without WinBoat",
    );
    recordAssertion(
      environment.ready,
      "the isolated native workspace is ready",
    );
    recordAssertion(
      await client.executeSync(
        "return !document.querySelector('.host-node') && !document.querySelector('.winboat-control > button');",
      ),
      "the native UI contains no Linux route or WinBoat control",
    );

    const config = await client.invoke("get_config");
    report.configSharedDirectory = config.sharedDirectory;
    recordAssertion(
      sameWindowsPath(config.sharedDirectory, primaryWorkspace),
      "the E2E build uses only its isolated workspace",
    );

    for (const command of [
      "start_winboat_windows",
      "open_winboat",
      "begin_winboat_setup",
      "complete_winboat_setup",
    ]) {
      const result = await client.invokeResult(command);
      recordAssertion(
        !result.ok && /command|not found|unknown/i.test(result.error),
        `${command} is not registered in the Windows binary`,
      );
    }

    const installedTimed = await timed(() =>
      client.invoke("get_installed_versions"),
    );
    report.measurements.installedDiscoveryMs = installedTimed.elapsedMs;
    report.measurements.installedVersions = installedTimed.value.length;
    assertThreshold("installedMs", installedTimed.elapsedMs);
    for (const version of installedTimed.value) {
      await access(version.executablePath);
    }
    recordAssertion(
      installedTimed.value.every(
        (version) =>
          typeof version.version === "string" &&
          typeof version.executablePath === "string",
      ),
      "every discovered Studio record points to an existing executable",
    );

    if (!SKIP_LIVE_STUDIO && installedTimed.value.length > 0) {
      const candidate = installedTimed.value.find(
        (version) =>
          windowsProcessesByExecutable(version.executablePath).length === 0,
      );
      if (candidate) {
        const nativeApp = windowsProcessSnapshot(port);
        const studioStarted = performance.now();
        await client.invoke("launch_studio_pro", {
          version: candidate.version,
          projectMprPath: null,
        });
        const launched = await waitUntil(
          () =>
            windowsProcessesByExecutable(candidate.executablePath).find(
              (processRecord) => processRecord.parentId === nativeApp.rootId,
            ),
          thresholds.studioLaunchMs,
          `Studio Pro ${candidate.version} process`,
        );
        launchedStudioProcessId = launched.id;
        const studioWindow = await waitUntil(
          () => windowsProcessWindow(launched.id),
          thresholds.studioLaunchMs,
          `Studio Pro ${candidate.version} native window`,
        );
        report.measurements.studioLaunchMs = rounded(
          performance.now() - studioStarted,
        );
        report.measurements.launchedStudioVersion = candidate.version;
        report.measurements.launchedStudioWindowTitle = studioWindow.title;
        assertThreshold("studioLaunchMs", report.measurements.studioLaunchMs);
        recordAssertion(
          studioWindow.handle !== "0",
          "a verified installed Studio Pro creates a native window",
        );
        terminateProcessTree(launchedStudioProcessId);
        launchedStudioProcessId = undefined;
      } else {
        report.measurements.liveStudioLaunch =
          "skipped because every installed version was already running";
      }
    }

    const projectsTimed = await timed(() => client.invoke("get_projects"));
    report.measurements.projectScanMs = projectsTimed.elapsedMs;
    assertThreshold("projectsMs", projectsTimed.elapsedMs);
    recordAssertion(
      projectsTimed.value.length === 1 &&
        projectsTimed.value[0].name === "Orders" &&
        projectsTimed.value[0].version === "11.12.2",
      "the real project scanner finds the isolated Orders fixture",
    );

    const projectsNavigation = await timed(async () => {
      await client.click("[data-testid=nav-projects]");
      await client.waitForSelector("[data-testid=projects-page]");
    });
    report.measurements.projectsNavigationMs = projectsNavigation.elapsedMs;
    assertThreshold("navigationMs", projectsNavigation.elapsedMs);
    recordAssertion(
      (await client.text("[data-testid=projects-page]")).includes("Orders"),
      "WebDriver navigation renders the scanned project in the real window",
    );

    const settingsNavigation = await timed(async () => {
      await client.click("[data-testid=nav-settings]");
      await client.waitForSelector("[data-testid=settings-native-page]");
    });
    report.measurements.settingsNavigationMs = settingsNavigation.elapsedMs;
    assertThreshold("navigationMs", settingsNavigation.elapsedMs);
    recordAssertion(
      !(await client.text("[data-testid=settings-native-page]")).includes(
        "WinBoat",
      ),
      "native settings expose no WinBoat configuration",
    );

    await client.clear("[data-testid=workspace-path] input");
    await client.sendKeys(
      "[data-testid=workspace-path] input",
      secondaryWorkspace,
    );
    await client.click("[data-testid=save-settings]");
    await waitUntil(
      async () => {
        const saved = await client.invoke("get_config");
        return sameWindowsPath(saved.sharedDirectory, secondaryWorkspace);
      },
      10_000,
      "native settings save",
    );
    recordAssertion(
      sameWindowsPath(
        (await client.invoke("get_config")).sharedDirectory,
        secondaryWorkspace,
      ),
      "the native settings UI persists a workspace through real IPC",
    );

    await client.click("[data-testid=nav-projects]");
    await client.waitForSelector("[data-testid=projects-page]");
    await client.click("[data-testid=refresh-projects]");
    await waitUntil(
      async () =>
        (await client.text("[data-testid=projects-page]")).includes(
          "Inventory",
        ),
      10_000,
      "project refresh after settings change",
    );
    recordAssertion(
      (await client.text("[data-testid=projects-page]")).includes("Inventory"),
      "project refresh uses the newly saved workspace",
    );

    await client.click("[data-testid=nav-studio]");
    await client.waitForSelector("[data-testid=studio-page]");
    if (!SKIP_LIVE_MARKETPLACE) {
      await waitUntil(
        async () => {
          const disabled = await client.attribute(
            "[data-testid=refresh-catalog]",
            "disabled",
          );
          const cache = await client.invoke("get_downloadable_versions_cache");
          return disabled === null && cache.versions.length > 0;
        },
        thresholds.marketplaceMs,
        "initial live Marketplace catalog",
      );
      const catalogRefresh = await timed(async () => {
        await client.click("[data-testid=refresh-catalog]");
        await waitUntil(
          async () =>
            (await client.attribute(
              "[data-testid=refresh-catalog]",
              "disabled",
            )) !== null,
          5_000,
          "catalog refresh start",
        );
        await waitUntil(
          async () =>
            (await client.attribute(
              "[data-testid=refresh-catalog]",
              "disabled",
            )) === null,
          thresholds.marketplaceMs,
          "catalog refresh completion",
        );
      });
      report.measurements.marketplaceRefreshMs = catalogRefresh.elapsedMs;
      assertThreshold("marketplaceMs", catalogRefresh.elapsedMs);
      const catalog = await client.invoke("get_downloadable_versions_cache");
      report.measurements.catalogVersions = catalog.versions.length;
      recordAssertion(
        catalog.versions.length > 0 && catalog.loadedPages.includes(1),
        "the real Edge-backed Marketplace refresh populates the isolated cache",
      );
    }

    for (const [command, payload] of [
      ["launch_studio_pro", { version: "11.12.2; calc.exe" }],
      ["uninstall_studio_pro", { version: "../11.12.2" }],
      ["open_folder", { path: join(isolatedRoot, "missing") }],
    ]) {
      const result = await client.invokeResult(command, payload);
      recordAssertion(
        !result.ok,
        `${command} rejects hostile or missing input`,
      );
    }

    const beforeIdle = windowsProcessSnapshot(port);
    await delay(10_000);
    const afterIdle = windowsProcessSnapshot(port);
    const idleCpuPercent =
      (Math.max(0, afterIdle.cpuSeconds - beforeIdle.cpuSeconds) /
        (10 * cpus().length)) *
      100;
    report.measurements.processId = afterIdle.rootId;
    report.measurements.processCount = afterIdle.processCount;
    report.measurements.privateMemoryBytes = afterIdle.privateMemoryBytes;
    report.measurements.workingSetBytes = afterIdle.workingSetBytes;
    report.measurements.idleCpuPercent = rounded(idleCpuPercent);
    recordAssertion(
      afterIdle.privateMemoryBytes <= thresholds.privateMemoryBytes,
      `private memory stays below ${thresholds.privateMemoryBytes} bytes`,
    );
    recordAssertion(
      idleCpuPercent <= thresholds.idleCpuPercent,
      `normalized idle CPU stays below ${thresholds.idleCpuPercent}%`,
    );

    const screenshot = await client.screenshot();
    await writeFile(
      join(ARTIFACT_DIRECTORY, "windows-native.png"),
      Buffer.from(screenshot, "base64"),
    );
    report.status = "passed";
  } catch (error) {
    failure = error;
    report.status = "failed";
    report.error = error instanceof Error ? error.stack : String(error);
    if (client?.sessionId) {
      try {
        const screenshot = await client.screenshot();
        await writeFile(
          join(ARTIFACT_DIRECTORY, "windows-native-failure.png"),
          Buffer.from(screenshot, "base64"),
        );
      } catch {
        // Preserve the primary failure when screenshot capture is unavailable.
      }
    }
  } finally {
    if (client?.sessionId) await client.deleteSession().catch(() => undefined);
    if (launchedStudioProcessId) terminateProcessTree(launchedStudioProcessId);
    if (appProcess?.pid) terminateProcessTree(appProcess.pid);
    await delay(1_000);
    await writeFile(
      join(ARTIFACT_DIRECTORY, "tauri-dev.log"),
      output.join(""),
      "utf8",
    );
    report.finishedAt = new Date().toISOString();
    await writeFile(
      join(ARTIFACT_DIRECTORY, "windows-native.json"),
      `${JSON.stringify(report, null, 2)}\n`,
      "utf8",
    );
    await removeIsolatedRoot(isolatedRoot);
  }

  if (failure) throw failure;
  process.stdout.write(
    `Native Windows E2E passed: ${report.assertions.length} assertions.\n`,
  );
}

class WebDriverClient {
  constructor(port) {
    this.baseUrl = `http://127.0.0.1:${port}`;
    this.sessionId = undefined;
  }

  async request(method, path, body, timeoutMs = 30_000) {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers:
        body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
    const text = await response.text();
    const parsed = text ? JSON.parse(text) : {};
    const webdriverError =
      parsed.value?.error &&
      parsed.value?.ok === undefined &&
      typeof parsed.value?.message === "string";
    if (!response.ok || webdriverError) {
      throw new Error(
        `WebDriver ${method} ${path} failed (${response.status}): ${text}`,
      );
    }
    return Object.hasOwn(parsed, "value") ? parsed.value : parsed;
  }

  async createSession() {
    const created = await this.request("POST", "/session", {
      capabilities: {
        alwaysMatch: {
          browserName: "tauri",
          "wdio:tauriServiceOptions": { windowLabel: "main" },
        },
      },
    });
    this.sessionId = created.sessionId;
    assert.ok(this.sessionId, "WebDriver did not return a session id");
  }

  async deleteSession() {
    await this.request("DELETE", `/session/${this.sessionId}`);
    this.sessionId = undefined;
  }

  async executeSync(script, args = []) {
    return this.request("POST", `/session/${this.sessionId}/execute/sync`, {
      script,
      args,
    });
  }

  async executeAsync(script, args = []) {
    return this.request(
      "POST",
      `/session/${this.sessionId}/execute/async`,
      { script, args },
      130_000,
    );
  }

  async invokeResult(command, payload = {}) {
    return this.executeAsync(
      `
        const done = arguments[arguments.length - 1];
        window.__TAURI__.core.invoke(arguments[0], arguments[1]).then(
          (value) => done({ ok: true, value }),
          (error) => done({
            ok: false,
            error: typeof error === 'string' ? error : JSON.stringify(error),
          }),
        );
      `,
      [command, payload],
    );
  }

  async invoke(command, payload = {}) {
    const result = await this.invokeResult(command, payload);
    if (!result.ok) throw new Error(`${command} failed: ${result.error}`);
    return result.value;
  }

  async find(selector) {
    const found = await this.request(
      "POST",
      `/session/${this.sessionId}/element`,
      { using: "css selector", value: selector },
    );
    return found[ELEMENT_KEY];
  }

  async waitForSelector(selector, timeoutMs = 10_000) {
    return waitUntil(
      async () => {
        try {
          return await this.find(selector);
        } catch {
          return false;
        }
      },
      timeoutMs,
      selector,
    );
  }

  async click(selector) {
    const element = await this.waitForSelector(selector);
    await this.request(
      "POST",
      `/session/${this.sessionId}/element/${encodeURIComponent(element)}/click`,
      {},
    );
  }

  async clear(selector) {
    const element = await this.waitForSelector(selector);
    await this.request(
      "POST",
      `/session/${this.sessionId}/element/${encodeURIComponent(element)}/clear`,
      {},
    );
  }

  async sendKeys(selector, text) {
    const element = await this.waitForSelector(selector);
    await this.request(
      "POST",
      `/session/${this.sessionId}/element/${encodeURIComponent(element)}/value`,
      { text, value: [...text] },
    );
  }

  async text(selector) {
    const element = await this.waitForSelector(selector);
    return this.request(
      "GET",
      `/session/${this.sessionId}/element/${encodeURIComponent(element)}/text`,
    );
  }

  async attribute(selector, name) {
    const element = await this.waitForSelector(selector);
    return this.request(
      "GET",
      `/session/${this.sessionId}/element/${encodeURIComponent(element)}/attribute/${encodeURIComponent(name)}`,
    );
  }

  screenshot() {
    return this.request("GET", `/session/${this.sessionId}/screenshot`);
  }
}

async function createProjectFixture(workspace, name, version) {
  const directory = join(workspace, name);
  await mkdir(directory, { recursive: true });
  await writeFile(
    join(directory, `${name}.mpr`),
    "native e2e fixture\n",
    "utf8",
  );
  await writeFile(
    join(directory, "project-settings.user.json"),
    `${JSON.stringify({
      settingsParts: [
        { type: `Mendix.Core, Version=${version}.0, Culture=neutral` },
      ],
    })}\n`,
    "utf8",
  );
}

async function availablePort() {
  const server = createServer();
  server.unref();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  const { port } = address;
  server.close();
  await once(server, "close");
  return port;
}

async function waitUntil(action, timeoutMs, description) {
  const started = performance.now();
  let lastError;
  while (performance.now() - started < timeoutMs) {
    try {
      const value = await action();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }
  throw new Error(
    `Timed out after ${timeoutMs} ms waiting for ${description}${
      lastError ? `: ${lastError}` : ""
    }`,
  );
}

async function timed(action) {
  const started = performance.now();
  const value = await action();
  return { value, elapsedMs: rounded(performance.now() - started) };
}

function windowsProcessSnapshot(port) {
  assert.ok(Number.isInteger(port) && port > 0 && port <= 65_535);
  const script = `
    $connection = Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction Stop |
      Select-Object -First 1
    $rootProcess = Get-Process -Id $connection.OwningProcess -ErrorAction Stop
    $allProcesses = @(Get-CimInstance Win32_Process -ErrorAction Stop)
    $processIds = [Collections.Generic.HashSet[uint32]]::new()
    [void]$processIds.Add([uint32]$rootProcess.Id)
    do {
      $added = $false
      foreach ($candidate in $allProcesses) {
        if (
          $processIds.Contains([uint32]$candidate.ParentProcessId) -and
          $processIds.Add([uint32]$candidate.ProcessId)
        ) {
          $added = $true
        }
      }
    } while ($added)
    $processes = @(
      $processIds |
        ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
    )
    [pscustomobject]@{
      rootId = $rootProcess.Id
      name = $rootProcess.ProcessName
      path = $rootProcess.Path
      processCount = $processes.Count
      cpuSeconds = ($processes | Measure-Object -Property CPU -Sum).Sum
      privateMemoryBytes = ($processes | Measure-Object -Property PrivateMemorySize64 -Sum).Sum
      workingSetBytes = ($processes | Measure-Object -Property WorkingSet64 -Sum).Sum
    } | ConvertTo-Json -Compress
  `;
  const snapshot = JSON.parse(
    execFileSync(
      "powershell.exe",
      ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
      { encoding: "utf8", windowsHide: true },
    ).trim(),
  );
  assert.equal(snapshot.name.toLowerCase(), "mendimaru");
  assert.match(
    snapshot.path.replaceAll("/", "\\").toLowerCase(),
    /\\src-tauri\\target\\debug\\mendimaru\.exe$/,
  );
  return snapshot;
}

function windowsProcessesByExecutable(executable) {
  const script = `
    $target = [IO.Path]::GetFullPath($env:MENDIMARU_E2E_PROCESS_PATH)
    Get-CimInstance Win32_Process -Filter "Name = 'StudioPro.exe'" -ErrorAction Stop |
      Where-Object {
        -not [string]::IsNullOrWhiteSpace($_.ExecutablePath) -and
        [IO.Path]::GetFullPath($_.ExecutablePath) -ieq $target
      } |
      ForEach-Object { "$($_.ProcessId)|$($_.ParentProcessId)" }
  `;
  const output = execFileSync(
    "powershell.exe",
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    {
      encoding: "utf8",
      windowsHide: true,
      env: { ...process.env, MENDIMARU_E2E_PROCESS_PATH: executable },
    },
  ).trim();
  if (!output) return [];
  return output.split(/\r?\n/).map((line) => {
    const [id, parentId] = line.trim().split("|").map(Number);
    assert.ok(Number.isInteger(id) && id > 0);
    assert.ok(Number.isInteger(parentId) && parentId >= 0);
    return { id, parentId };
  });
}

function windowsProcessWindow(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  const script = `
    $process = Get-Process -Id ${pid} -ErrorAction SilentlyContinue
    if ($null -eq $process) { exit 0 }
    $process.Refresh()
    [pscustomobject]@{
      handle = $process.MainWindowHandle.ToString()
      title = $process.MainWindowTitle
    } | ConvertTo-Json -Compress
  `;
  const output = execFileSync(
    "powershell.exe",
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    { encoding: "utf8", windowsHide: true },
  ).trim();
  if (!output) return false;
  const snapshot = JSON.parse(output);
  return snapshot.handle === "0" ? false : snapshot;
}

function terminateProcessTree(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return;
  try {
    execFileSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true,
    });
  } catch {
    // The dev process may already have exited after the WebDriver session closed.
  }
}

async function removeIsolatedRoot(path) {
  const canonicalTemporary = await realpath(tmpdir());
  const canonicalRoot = await realpath(path);
  const relativePath = relative(canonicalTemporary, canonicalRoot);
  const marker = await readFile(join(canonicalRoot, MARKER_NAME), "utf8");
  if (
    !relativePath ||
    relativePath.startsWith(`..${sep}`) ||
    dirname(canonicalRoot) !== canonicalTemporary ||
    !basename(canonicalRoot).startsWith("mendimaru-e2e-") ||
    marker !== MARKER_CONTENT
  ) {
    throw new Error(
      `Refusing to remove unsafe E2E directory: ${canonicalRoot}`,
    );
  }
  await rm(canonicalRoot, { recursive: true, maxRetries: 3, retryDelay: 250 });
}

function captureOutput(stream, collection, destination) {
  stream?.on("data", (chunk) => {
    const text = chunk.toString();
    collection.push(text);
    destination.write(text);
  });
}

function recordAssertion(condition, message) {
  assert.ok(condition, message);
  report.assertions.push(message);
}

function assertThreshold(name, value) {
  const threshold = thresholds[name];
  recordAssertion(value <= threshold, `${name} ${value} ms <= ${threshold} ms`);
}

function numberEnvironment(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive number`);
  }
  return parsed;
}

function sameWindowsPath(left, right) {
  return (
    left.replaceAll("/", "\\").toLowerCase() ===
    right.replaceAll("/", "\\").toLowerCase()
  );
}

function rounded(value) {
  return Math.round(value * 100) / 100;
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

await run();
