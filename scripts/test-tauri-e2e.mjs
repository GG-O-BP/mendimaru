import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { constants as fsConstants, promises as fs } from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const fetch = globalThis.fetch;

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const application = path.join(
  repository,
  "src-tauri",
  "target",
  "debug",
  "mendimaru",
);
const viteUrl = "http://localhost:1420/";
const httpProbeTimeout = 2_000;
const webdriverRequestTimeout = 30_000;
const buildTimeout = 15 * 60_000;
const artifactDirectory = path.join(repository, "artifacts", "e2e");
const thresholds = {
  startupMs: numberEnvironment("MENDIMARU_E2E_MAX_STARTUP_MS", 120_000),
  environmentMs: numberEnvironment("MENDIMARU_E2E_MAX_ENVIRONMENT_MS", 5_000),
  projectsMs: numberEnvironment("MENDIMARU_E2E_MAX_PROJECTS_MS", 10_000),
  navigationMs: numberEnvironment("MENDIMARU_E2E_MAX_NAVIGATION_MS", 3_000),
  privateMemoryBytes: numberEnvironment(
    "MENDIMARU_E2E_MAX_PRIVATE_MEMORY_BYTES",
    768 * 1024 * 1024,
  ),
  idleCpuPercent: numberEnvironment("MENDIMARU_E2E_MAX_IDLE_CPU_PERCENT", 10),
};
const processes = [];
const servers = [];
const serverSockets = new Map();
const report = {
  status: "running",
  startedAt: new Date().toISOString(),
  node: process.version,
  platform: `${process.platform}-${process.arch}`,
  thresholds,
  measurements: {},
  assertions: [],
};
let sessionId;
let webdriverUrl;
let temporary;
let succeeded = false;
let clockTicks;

try {
  await fs.mkdir(artifactDirectory, { recursive: true });
  await Promise.all(
    ["linux-tauri.json", "linux-tauri.png", "linux-tauri-failure.png"].map(
      (name) => fs.rm(path.join(artifactDirectory, name), { force: true }),
    ),
  );
  assert.equal(
    process.platform,
    "linux",
    "the real Tauri WebKit E2E currently runs on Linux only",
  );
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

  await runChecked("cargo", [
    "build",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--no-default-features",
  ]);
  await assertPortUnused(1420, "Vite development server");

  temporary = await fs.mkdtemp(path.join(os.tmpdir(), "mendimaru-tauri-e2e-"));
  const fixture = await createFixture(temporary);
  const driverPort = await unusedPort();
  const nativeDriverPort = await unusedPort();
  webdriverUrl = `http://127.0.0.1:${driverPort}`;

  const vite = startProcess("npm", ["run", "dev"], {
    cwd: repository,
    label: "Vite",
    env: { ...process.env, VITE_MENDIMARU_E2E: "1" },
  });
  processes.push(vite);
  await waitForHttp(viteUrl, vite, 20_000);

  const driver = startProcess(
    tauriDriver,
    [
      "--port",
      String(driverPort),
      "--native-port",
      String(nativeDriverPort),
      "--native-driver",
      webkitDriver,
    ],
    {
      cwd: repository,
      env: {
        ...process.env,
        MENDIMARU_CHROME_PATH: fixture.chrome,
        PATH: `${fixture.bin}${path.delimiter}${process.env.PATH ?? ""}`,
        XDG_CACHE_HOME: fixture.xdgCache,
        XDG_CONFIG_HOME: fixture.xdgConfig,
      },
      label: "tauri-driver",
    },
  );
  processes.push(driver);
  await waitForWebDriver(webdriverUrl, driver, 20_000);

  const startupStarted = performance.now();
  sessionId = await createSession(webdriverUrl, application);
  await waitFor(
    async () =>
      (await execute("return document.readyState;")) === "complete" &&
      (await execute(
        "return document.querySelector('main h1')?.textContent;",
      )) === "Studio Pro",
    30_000,
    "the Tauri application shell did not become ready",
  );
  report.measurements.startupMs = rounded(performance.now() - startupStarted);
  assertThreshold("startupMs", report.measurements.startupMs);
  await execute(`
    window.__MENDIMARU_E2E_ERRORS__ = [];
    window.addEventListener("error", (event) => {
      window.__MENDIMARU_E2E_ERRORS__.push(String(event.error ?? event.message));
    });
    window.addEventListener("unhandledrejection", (event) => {
      window.__MENDIMARU_E2E_ERRORS__.push(String(event.reason));
    });
    return true;
  `);

  await waitFor(
    async () =>
      await execute(`
        const installRow = Array.from(document.querySelectorAll(".manifest-table tbody tr"))
          .find((row) => row.querySelector(".version-cell strong")?.textContent === "11.13.0");
        return document.body.innerText.includes("11.12.2")
          && document.body.innerText.includes(
            "Showing the last known installation list while Windows verifies it.",
          )
          && installRow?.querySelector(".manifest-action .button")?.disabled === true
          && !installRow?.querySelector('[aria-label="Force a fresh download"]');
      `),
    1_500,
    "the cached installed list was not rendered safely before live detection",
  );

  assert.match(
    await command("GET", "/title"),
    /^mendimaru — Mendix Studio Pro Manager$/,
  );
  assert.equal(await execute("return location.href;"), viteUrl);
  assert.equal(
    await execute("return Boolean(window.__TAURI_INTERNALS__);"),
    true,
  );
  const csp = await execute(
    "return document.querySelector('meta[http-equiv=\"Content-Security-Policy\"]')?.content || '';",
  );
  report.cspMeta = csp;
  recordAssertion(
    csp.includes("object-src 'none'") && csp.includes("base-uri 'none'"),
    "the real Linux WebView receives the restrictive development CSP",
  );
  const dataScriptBlocked = await executeAsync(`
    const done = arguments[arguments.length - 1];
    Promise.resolve(window.__MENDIMARU_CSP_PROBE__).then(done, () => done(false));
  `);
  recordAssertion(
    dataScriptBlocked,
    "the real Linux WebView blocks a data-script CSP probe",
  );

  const capabilities = await executeAsync(`
    const done = arguments[arguments.length - 1];
    window.__TAURI_INTERNALS__.invoke("get_capabilities", { backend: null })
      .then((value) => done({ ok: true, value }))
      .catch((error) => done({ ok: false, error: String(error) }));
  `);
  assert.equal(capabilities.ok, true, capabilities.error);
  assert.equal(capabilities.value.schemaVersion, "3.0.0");
  assert.equal(capabilities.value.manifest.hostPlatform, "linux");
  assert.equal(capabilities.value.manifest.backend, "linux-winboat");

  const environmentTimed = await timed(() => invoke("get_environment_status"));
  report.measurements.environmentMs = environmentTimed.elapsedMs;
  assertThreshold("environmentMs", environmentTimed.elapsedMs);
  recordAssertion(
    environmentTimed.value.platform.kind === "linux-winboat" &&
      environmentTimed.value.platform.requiresWinboat === true &&
      environmentTimed.value.ready === true,
    "the actual Linux backend reports a ready WinBoat environment",
  );

  const config = await invoke("get_config");
  report.configSharedDirectory = config.sharedDirectory;
  report.expectedSharedDirectory = fixture.shared;
  recordAssertion(
    await samePath(config.sharedDirectory, fixture.shared),
    "the Linux E2E uses only its isolated shared workspace",
  );

  const projectsTimed = await timed(() => invoke("get_projects"));
  report.measurements.projectScanMs = projectsTimed.elapsedMs;
  assertThreshold("projectsMs", projectsTimed.elapsedMs);
  recordAssertion(
    projectsTimed.value.length === 1 &&
      projectsTimed.value[0].name === "Orders" &&
      projectsTimed.value[0].version === "11.12.2",
    "the real Linux project scanner finds the isolated Orders fixture",
  );

  await waitFor(
    async () =>
      await execute(`
        const installRow = Array.from(document.querySelectorAll(".manifest-table tbody tr"))
          .find((row) => row.querySelector(".version-cell strong")?.textContent === "11.13.0");
        return document.querySelector(".route-status")?.classList.contains("online")
          && document.body.innerText.includes("WinBoat online")
          && document.body.innerText.includes("11.12.2")
          && document.body.innerText.includes("11.13.0")
          && installRow?.querySelector(".manifest-action .button")?.disabled === false;
      `),
    20_000,
    "the fixture-backed native commands did not populate the online Studio view",
  );

  const routeMotion = await execute(`
    const marker = document.querySelector(".route-packet");
    if (!marker) return null;
    const style = getComputedStyle(marker);
    const animation = marker.getAnimations()[0];
    return {
      animationName: style.animationName,
      iterationCount: style.animationIterationCount,
      playState: animation?.playState ?? null,
      currentTime: animation?.currentTime ?? null,
    };
  `);
  assert.equal(routeMotion?.animationName, "route-travel");
  assert.equal(routeMotion?.iterationCount, "infinite");
  assert.equal(routeMotion?.playState, "running");
  assert.equal(typeof routeMotion?.currentTime, "number");

  const routeFrames = [];
  for (let sample = 0; sample < 8; sample += 1) {
    routeFrames.push(
      await execute(`
        const marker = document.querySelector(".route-status.online .route-packet");
        if (!marker) return null;
        const animation = marker.getAnimations()[0];
        const style = getComputedStyle(marker);
        return {
          currentTime: animation?.currentTime ?? null,
          opacity: style.opacity,
          transform: style.transform,
        };
      `),
    );
    await delay(120);
  }
  assert(
    routeFrames.every(
      (frame) => frame && typeof frame.currentTime === "number",
    ),
    "the online route animation must remain attached for every sampled frame",
  );
  assert(
    routeFrames.at(-1).currentTime > routeFrames[0].currentTime + 500,
    "the online route animation timeline did not advance",
  );
  assert(
    new Set(routeFrames.map((frame) => frame.transform)).size >= 4,
    `the online route marker did not visibly move: ${JSON.stringify(routeFrames)}`,
  );

  const activeMotion = await execute(`
    const probe = document.createElement("div");
    probe.id = "mendimaru-e2e-motion-probe";
    probe.setAttribute("aria-hidden", "true");
    probe.innerHTML = \`
      <div class="spin"></div>
      <div class="download-bar" aria-busy="true">
        <div class="progress-track"><span class="active"></span></div>
        <div class="progress-stages"><span class="current"><i></i></span></div>
      </div>
    \`;
    document.body.append(probe);
    const result = {
      spinner: getComputedStyle(probe.querySelector(".spin")).animationName,
      shimmer: getComputedStyle(
        probe.querySelector(".progress-track > span"),
        "::after",
      ).animationName,
      stagePulse: getComputedStyle(
        probe.querySelector(".progress-stages i"),
      ).animationName,
    };
    return result;
  `);
  assert.deepEqual(
    activeMotion,
    {
      spinner: "spin",
      shimmer: "progress-shimmer",
      stagePulse: "progress-stage-pulse",
    },
    "busy and installation states must retain visible motion",
  );

  const busyFrames = [];
  for (let sample = 0; sample < 8; sample += 1) {
    busyFrames.push(
      await execute(`
        const probe = document.querySelector("#mendimaru-e2e-motion-probe");
        if (!probe) return null;
        const spinner = getComputedStyle(probe.querySelector(".spin"));
        const shimmer = getComputedStyle(
          probe.querySelector(".progress-track > span"),
          "::after",
        );
        const stage = getComputedStyle(
          probe.querySelector(".progress-stages i"),
        );
        return {
          spinnerTransform: spinner.transform,
          shimmerTransform: shimmer.transform,
          stageTransform: stage.transform,
          stageOpacity: stage.opacity,
        };
      `),
    );
    await delay(120);
  }
  assert(
    busyFrames.every(Boolean),
    "the busy-motion probe disappeared while frames were sampled",
  );
  for (const property of [
    "spinnerTransform",
    "shimmerTransform",
    "stageTransform",
  ]) {
    assert(
      new Set(busyFrames.map((frame) => frame[property])).size >= 4,
      `${property} did not visibly change: ${JSON.stringify(busyFrames)}`,
    );
  }
  assert(
    new Set(busyFrames.map((frame) => frame.stageOpacity)).size >= 4,
    `stageOpacity did not visibly change: ${JSON.stringify(busyFrames)}`,
  );
  await execute(`
    document.querySelector("#mendimaru-e2e-motion-probe")?.remove();
    return true;
  `);

  await delay(1_100);

  const idleMotion = await execute(`
    const infiniteAnimations = document.getAnimations()
      .filter((animation) => animation.effect?.getTiming().iterations === Infinity)
      .map((animation) => ({
        animationName: getComputedStyle(animation.effect.target).animationName,
        isOnlineRoute: animation.effect.target.matches(
          ".route-status.online .route-packet",
        ),
      }));
    return {
      infiniteAnimations,
      unexpectedRunningAnimations: document.getAnimations()
        .filter((animation) => {
          const target = animation.effect?.target;
          const name = target ? getComputedStyle(target).animationName : "";
          const allowlisted =
            name === "route-travel" &&
            target instanceof Element &&
            target.matches(".route-status.online .route-packet");
          return animation.playState === "running" && !allowlisted;
        })
        .map((animation) => ({
          animationName: animation.effect?.target
            ? getComputedStyle(animation.effect.target).animationName
            : "",
          className: animation.effect?.target?.className ?? "",
        })),
      routeMarkerPresent: Boolean(document.querySelector(".route-packet")),
    };
  `);
  assert.deepEqual(
    idleMotion,
    {
      infiniteAnimations: [
        { animationName: "route-travel", isOnlineRoute: true },
      ],
      unexpectedRunningAnimations: [],
      routeMarkerPresent: true,
    },
    "the idle native window may retain only the live online-route indicator",
  );

  const installedRefresh = await command("POST", "/element", {
    using: "css selector",
    value: ".installed-section .icon-button",
  });
  const installedRefreshId =
    installedRefresh["element-6066-11e4-a52e-4f735466cecf"];
  assert(
    installedRefreshId,
    "WebDriver did not find the installed refresh action",
  );
  await command("POST", `/element/${installedRefreshId}/click`, {});
  await waitFor(
    async () =>
      await execute(`
        const spinner = document.querySelector(".installed-section .spin");
        return spinner
          && getComputedStyle(spinner).animationName === "spin"
          && document.querySelector(".installed-section .icon-button")?.disabled;
      `),
    5_000,
    "the real installed-version refresh never exposed its busy animation",
  );
  await waitFor(
    async () =>
      await execute(`
        return !document.querySelector(".installed-section .spin")
          && !document.querySelector(".installed-section .icon-button")?.disabled;
      `),
    5_000,
    "the installed-version busy animation did not stop after refresh",
  );

  for (const [name, shortcut, heading, expectedText] of [
    ["projectsNavigationMs", "Control+2", "Projects", "Orders"],
    ["operationsNavigationMs", "Control+3", "Operation center", "11.12.2"],
    [
      "settingsNavigationMs",
      "Control+4",
      "Settings",
      "Environment diagnostics",
    ],
    ["studioNavigationMs", "Control+1", "Studio Pro", "11.12.2"],
  ]) {
    const elapsedMs = await assertView(shortcut, heading, expectedText);
    report.measurements[name] = elapsedMs;
    assertThreshold("navigationMs", elapsedMs);
  }

  for (const [commandName, payload] of [
    ["launch_studio_pro", { version: "11.12.2; calc.exe" }],
    ["uninstall_studio_pro", { version: "../11.12.2" }],
    ["open_folder", { path: path.join(temporary, "missing") }],
  ]) {
    const result = await invokeResult(commandName, payload);
    recordAssertion(
      !result.ok,
      `${commandName} rejects hostile or missing input on Linux`,
    );
  }

  await delay(1_750);
  assert.deepEqual(
    await execute("return window.__MENDIMARU_E2E_ERRORS__;"),
    [],
    "the real WebView reported an error during the application flow",
  );

  const applicationPid = await findApplicationProcess(
    application,
    fixture.xdgConfig,
  );
  const idleStarted = performance.now();
  const beforeIdle = await linuxProcessSnapshot(applicationPid);
  await delay(10_000);
  const afterIdle = await linuxProcessSnapshot(applicationPid);
  const idleSeconds = (performance.now() - idleStarted) / 1_000;
  const idleCpuPercent =
    (Math.max(0, afterIdle.cpuSeconds - beforeIdle.cpuSeconds) /
      (idleSeconds * os.cpus().length)) *
    100;
  report.measurements.processId = applicationPid;
  report.measurements.processCount = afterIdle.processCount;
  report.measurements.privateMemoryBytes = afterIdle.privateMemoryBytes;
  report.measurements.workingSetBytes = afterIdle.workingSetBytes;
  report.measurements.idleCpuPercent = rounded(idleCpuPercent);
  recordAssertion(
    afterIdle.privateMemoryBytes <= thresholds.privateMemoryBytes,
    `Linux private memory stays below ${thresholds.privateMemoryBytes} bytes`,
  );
  recordAssertion(
    idleCpuPercent <= thresholds.idleCpuPercent,
    `normalized Linux idle CPU stays below ${thresholds.idleCpuPercent}%`,
  );

  const screenshot = await command("GET", "/screenshot");
  await fs.writeFile(
    path.join(artifactDirectory, "linux-tauri.png"),
    Buffer.from(screenshot, "base64"),
  );
  report.status = "passed";
  succeeded = true;
} catch (error) {
  report.status = "failed";
  report.error = error instanceof Error ? error.stack : String(error);
  await retainFailureScreenshot();
  throw error;
} finally {
  if (sessionId && webdriverUrl) {
    await webdriverRequest("DELETE", `/session/${sessionId}`).catch(
      () => undefined,
    );
  }
  for (const child of processes.reverse()) await terminate(child);
  for (const server of servers.reverse()) await closeServer(server);
  if (temporary && succeeded) {
    await fs.rm(temporary, { force: true, recursive: true });
  } else if (temporary) {
    process.stderr.write(`Tauri E2E evidence retained at ${temporary}\n`);
  }
  report.finishedAt = new Date().toISOString();
  await fs.writeFile(
    path.join(artifactDirectory, "linux-tauri.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
}

if (succeeded) {
  process.stdout.write(
    `Tauri E2E: real WebKit window passed (functional, security, and performance; ${report.assertions.length} explicit parity assertions)\n`,
  );
}

async function assertView(shortcut, heading, expectedText) {
  const started = performance.now();
  const selector = `button[aria-keyshortcuts="${shortcut}"]`;
  const element = await command("POST", "/element", {
    using: "css selector",
    value: selector,
  });
  const elementId = element["element-6066-11e4-a52e-4f735466cecf"];
  assert(elementId, `WebDriver did not return an element for ${selector}`);
  await command("POST", `/element/${elementId}/click`, {});
  await waitFor(
    async () =>
      (await execute(
        "return document.querySelector('main h1')?.textContent;",
      )) === heading &&
      (await execute("return document.querySelector('main')?.innerText;"))
        .toString()
        .includes(expectedText),
    10_000,
    `${heading} did not render through the real Tauri window`,
  );
  assert.equal(
    await command("GET", `/element/${elementId}/attribute/aria-current`),
    "page",
  );
  return rounded(performance.now() - started);
}

async function createSession(baseUrl, binary) {
  const value = await webdriverRequest(
    "POST",
    "/session",
    {
      capabilities: {
        alwaysMatch: {
          browserName: "wry",
          "tauri:options": { application: binary },
        },
      },
    },
    60_000,
  );
  assert(value.sessionId, "tauri-driver did not return a WebDriver session");
  return value.sessionId;
}

async function command(method, endpoint, body) {
  return webdriverRequest(method, `/session/${sessionId}${endpoint}`, body);
}

async function execute(script, args = []) {
  return command("POST", "/execute/sync", { args, script });
}

async function executeAsync(script, args = []) {
  return command("POST", "/execute/async", { args, script });
}

async function invokeResult(commandName, payload = {}) {
  return executeAsync(
    `
      const done = arguments[arguments.length - 1];
      window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(
        (value) => done({ ok: true, value }),
        (error) => done({
          ok: false,
          error: typeof error === "string" ? error : JSON.stringify(error),
        }),
      );
    `,
    [commandName, payload],
  );
}

async function invoke(commandName, payload = {}) {
  const result = await invokeResult(commandName, payload);
  if (!result.ok) throw new Error(`${commandName} failed: ${result.error}`);
  return result.value;
}

async function webdriverRequest(
  method,
  endpoint,
  body,
  timeout = webdriverRequestTimeout,
) {
  const response = await fetch(`${webdriverUrl}${endpoint}`, {
    method,
    headers:
      body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: globalThis.AbortSignal.timeout(timeout),
  });
  const envelope = await response.json();
  const webdriverError =
    envelope.value?.error &&
    envelope.value?.ok === undefined &&
    typeof envelope.value?.message === "string";
  if (!response.ok || webdriverError) {
    throw new Error(
      `WebDriver ${method} ${endpoint} failed: ${JSON.stringify(envelope.value)}`,
    );
  }
  return envelope.value;
}

async function createFixture(root) {
  const bin = path.join(root, "bin");
  const shared = path.join(root, "shared");
  const xdgCache = path.join(root, "cache");
  const xdgConfig = path.join(root, "config");
  await Promise.all([
    fs.mkdir(bin, { mode: 0o700, recursive: true }),
    fs.mkdir(path.join(shared, "Orders"), { mode: 0o700, recursive: true }),
    fs.mkdir(path.join(xdgCache, "com.ggobp.mendimaru"), {
      mode: 0o700,
      recursive: true,
    }),
    fs.mkdir(path.join(xdgConfig, "com.ggobp.mendimaru"), {
      mode: 0o700,
      recursive: true,
    }),
  ]);

  const api = http.createServer(async (request, response) => {
    response.setHeader("content-type", "application/json");
    if (request.url === "/health") {
      response.end('{"status":"ok"}');
      return;
    }
    if (request.url === "/apps") {
      await delay(2_000);
      response.end(
        JSON.stringify([
          {
            Name: "Studio Pro",
            Path: String.raw`C:\Program Files\Mendix\11.12.2\modeler\studiopro.exe`,
            Args: "",
            Icon: "",
            Source: "Tauri E2E fixture",
          },
        ]),
      );
      return;
    }
    response.statusCode = 404;
    response.end('{"error":"not-found"}');
  });
  const apiPort = await listen(api);
  servers.push(api);

  const rdp = net.createServer((socket) => socket.end());
  const rdpPort = await listen(rdp);
  servers.push(rdp);

  const inspectPayload = [
    {
      State: { Status: "running" },
      Mounts: [
        { Source: shared, Destination: "/shared" },
        { Source: "mendimaru-e2e-storage", Destination: "/storage" },
      ],
      NetworkSettings: {
        Ports: {
          "7148/tcp": [{ HostIp: "127.0.0.1", HostPort: String(apiPort) }],
          "3389/tcp": [{ HostIp: "127.0.0.1", HostPort: String(rdpPort) }],
        },
      },
    },
  ];
  const docker = path.join(bin, "docker");
  await writeExecutable(
    docker,
    `#!${process.execPath}\nconst args = process.argv.slice(2);\nconst inspect = ${JSON.stringify(inspectPayload)};\nif (args[0] === "info") { console.log("29.7.2"); process.exit(0); }\nif (args[0] === "port") {\n  const port = args[2] === "7148/tcp" ? ${apiPort} : args[2] === "3389/tcp" ? ${rdpPort} : 0;\n  if (port) { console.log("127.0.0.1:" + port); process.exit(0); }\n}\nif (args[0] === "inspect") { console.log(JSON.stringify(inspect)); process.exit(0); }\nprocess.exit(1);\n`,
  );
  const freerdp = path.join(bin, "xfreerdp3");
  await writeExecutable(
    freerdp,
    `#!${process.execPath}\nif (process.argv.includes("/version")) { console.log("FreeRDP version 3.30.0"); process.exit(0); }\nsetTimeout(() => process.exit(1), 600);\n`,
  );
  const chrome = path.join(bin, "google-chrome-stable");
  await writeExecutable(
    chrome,
    `#!${process.execPath}\nif (process.argv.includes("--version")) { console.log("Chromium 151.0.7922.34"); process.exit(0); }\nprocess.exit(1);\n`,
  );
  const winboat = path.join(bin, "winboat");
  await writeExecutable(winboat, `#!${process.execPath}\nprocess.exit(0);\n`);

  const compose = path.join(root, "compose.yml");
  await fs.writeFile(
    compose,
    `services:\n  windows:\n    image: mendimaru-e2e-fixture\n    container_name: MendimaruTauriE2E\n    volumes:\n      - ${JSON.stringify(`${shared}:/shared`)}\n      - mendimaru-e2e-storage:/storage\n    ports:\n      - 127.0.0.1:${apiPort}:7148\n      - 127.0.0.1:${rdpPort}:3389\nvolumes:\n  mendimaru-e2e-storage: {}\n`,
  );
  await fs.writeFile(path.join(shared, "Orders", "Orders.mpr"), "fixture\n");
  await fs.writeFile(
    path.join(shared, "Orders", "project-settings.user.json"),
    `${JSON.stringify({
      settingsParts: [
        { type: "Mendix.Core, Version=11.12.2.0, Culture=neutral" },
      ],
    })}\n`,
  );

  await fs.writeFile(
    path.join(xdgConfig, "com.ggobp.mendimaru", "config.json"),
    `${JSON.stringify(
      {
        languagePreference: "en-US",
        winboatSetupPending: false,
        winboatExecutable: winboat,
        composeFile: compose,
        containerRuntime: "docker",
        containerName: "MendimaruTauriE2E",
        apiUrl: `http://127.0.0.1:${apiPort}`,
        rdpHost: "127.0.0.1",
        rdpPort,
        sharedDirectory: shared,
        windowsSharedDirectory: String.raw`\\host.lan\Data`,
        freerdpBinary: freerdp,
        mendixInstallRoot: String.raw`C:\Program Files\Mendix`,
        mendixDataRoot: String.raw`C:\ProgramData\Mendix`,
        windowsStudioPaths: [],
        startupTimeoutSeconds: 2,
      },
      null,
      2,
    )}\n`,
  );
  await fs.writeFile(
    path.join(xdgConfig, "com.ggobp.mendimaru", "operation-history.json"),
    `${JSON.stringify(
      {
        schemaVersion: "1.0.0",
        records: [
          {
            schemaVersion: "1.0.0",
            id: "install-11.12.2-0123456789abcdef0123456789abcdef",
            kind: "install",
            targetVersion: "11.12.2",
            protectedProject: false,
            state: "succeeded",
            stage: "completed",
            percentage: 100,
            estimated: false,
            startedAt: "2026-08-14T00:00:00Z",
            updatedAt: "2026-08-14T00:01:00Z",
            finishedAt: "2026-08-14T00:01:00Z",
            retryable: false,
            logAvailable: false,
          },
        ],
        legacyScanComplete: true,
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  await fs.writeFile(
    path.join(xdgCache, "com.ggobp.mendimaru", "studio-version-catalog.json"),
    `${JSON.stringify(
      {
        versions: [
          {
            version: "11.13.0",
            releaseDate: "2026-07-28",
            releaseNotesUrl: null,
            isLts: false,
            isBeta: false,
            isMts: true,
            isLatest: true,
          },
        ],
        loadedPages: [1],
        totalCount: 1,
        fetchedAt: "2026-08-15T00:00:00Z",
      },
      null,
      2,
    )}\n`,
  );
  await fs.writeFile(
    path.join(
      xdgCache,
      "com.ggobp.mendimaru",
      "installed-studio-versions.json",
    ),
    `${JSON.stringify(
      {
        schemaVersion: "1.0.0",
        sourceIdentity: installedCacheSourceIdentity({
          containerRuntime: "docker",
          containerName: "MendimaruTauriE2E",
          apiUrl: `http://127.0.0.1:${apiPort}`,
          mendixInstallRoot: String.raw`C:\Program Files\Mendix`,
          mendixDataRoot: String.raw`C:\ProgramData\Mendix`,
          windowsStudioPaths: [],
        }),
        capturedAt: "2026-08-15T00:00:00Z",
        versions: [
          {
            version: "11.12.2",
            displayName: "Studio Pro",
            executablePath: String.raw`C:\Program Files\Mendix\11.12.2\modeler\studiopro.exe`,
            installRoot: String.raw`C:\Program Files\Mendix\11.12.2`,
            source: "Tauri E2E fixture cache",
            removable: true,
          },
        ],
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );

  return { bin, chrome, shared, xdgCache, xdgConfig };
}

function installedCacheSourceIdentity(config) {
  const hash = createHash("sha256");
  for (const value of [
    process.platform,
    config.containerRuntime,
    config.containerName,
    config.apiUrl,
    config.mendixInstallRoot,
    config.mendixDataRoot,
    ...config.windowsStudioPaths,
  ]) {
    hash.update(value);
    hash.update(Buffer.from([0]));
  }
  return hash.digest("hex");
}

async function writeExecutable(file, content) {
  await fs.writeFile(file, content, { mode: 0o700 });
  await fs.chmod(file, 0o700);
}

function startProcess(commandName, args, options) {
  const child = spawn(commandName, args, {
    cwd: options.cwd,
    // Vite and tauri-driver both create grandchildren.  A dedicated process
    // group lets cleanup stop the complete tree instead of only its launcher.
    detached: true,
    env: options.env ?? process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.label = options.label;
  child.output = "";
  child.spawnError = undefined;
  child.once("error", (error) => {
    child.spawnError = error;
  });
  for (const stream of [child.stdout, child.stderr]) {
    stream.on("data", (chunk) => {
      child.output = `${child.output}${chunk}`.slice(-64 * 1024);
    });
  }
  return child;
}

async function runChecked(commandName, args) {
  const child = startProcess(commandName, args, {
    cwd: repository,
    label: commandName,
  });
  const code = await raceTimeout(onceExit(child), buildTimeout);
  if (code === "timeout") {
    await terminate(child);
    assert.fail(`${commandName} exceeded ${buildTimeout}ms:\n${child.output}`);
  }
  assert.equal(code, 0, `${commandName} failed:\n${child.output}`);
}

async function waitForHttp(url, child, timeout) {
  await waitFor(
    async () => {
      assertRunning(child);
      try {
        return (
          await fetch(url, {
            signal: globalThis.AbortSignal.timeout(httpProbeTimeout),
          })
        ).ok;
      } catch {
        return false;
      }
    },
    timeout,
    `${child.label} did not become ready:\n${child.output}`,
  );
}

async function waitForWebDriver(url, child, timeout) {
  await waitFor(
    async () => {
      assertRunning(child);
      try {
        const response = await fetch(`${url}/status`, {
          signal: globalThis.AbortSignal.timeout(httpProbeTimeout),
        });
        return response.ok;
      } catch {
        return false;
      }
    },
    timeout,
    `${child.label} did not become ready:\n${child.output}`,
  );
}

async function waitFor(predicate, timeout, message) {
  const deadline = Date.now() + timeout;
  let lastError;
  while (Date.now() < deadline) {
    try {
      if (await predicate()) return;
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  throw new Error(`${message}${lastError ? `: ${lastError.message}` : ""}`);
}

function assertRunning(child) {
  if (child.spawnError) {
    throw new Error(
      `${child.label} failed to start: ${child.spawnError.message}`,
    );
  }
  if (childFinished(child)) {
    throw new Error(
      `${child.label} exited with ${child.exitCode ?? child.signalCode}:\n${child.output}`,
    );
  }
}

async function assertPortUnused(port, label) {
  const used = await new Promise((resolve) => {
    const socket = net.connect({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
  assert.equal(used, false, `${label} port ${port} is already in use`);
}

async function unusedPort() {
  const server = net.createServer();
  const port = await listen(server);
  await closeServer(server);
  return port;
}

async function listen(server) {
  const sockets = new Set();
  serverSockets.set(server, sockets);
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return server.address().port;
}

async function closeServer(server) {
  if (!server.listening) {
    serverSockets.delete(server);
    return;
  }
  const closed = new Promise((resolve) => {
    server.close(resolve);
  });
  for (const socket of serverSockets.get(server) ?? []) socket.destroy();
  server.closeAllConnections?.();
  const status = await raceTimeout(closed, 2_000);
  serverSockets.delete(server);
  assert.notEqual(status, "timeout", "fixture server did not close cleanly");
}

async function terminate(child) {
  if (!child) return;
  try {
    if (processTreeRunning(child)) {
      signalProcessTree(child, "SIGTERM");
      if (!(await waitForProcessTreeExit(child, 2_000))) {
        signalProcessTree(child, "SIGKILL");
        assert.equal(
          await waitForProcessTreeExit(child, 2_000),
          true,
          `${child.label} process tree did not terminate`,
        );
      }
    }
  } finally {
    // A surviving grandchild can otherwise keep these pipe handles open and
    // prevent Node from exiting even after every assertion has passed.
    child.stdout?.destroy();
    child.stderr?.destroy();
  }
}

function signalProcessTree(child, signal) {
  try {
    process.kill(-child.pid, signal);
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function processTreeRunning(child) {
  if (!child.pid) return false;
  try {
    process.kill(-child.pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

async function waitForProcessTreeExit(child, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (!processTreeRunning(child)) return true;
    await delay(25);
  }
  return !processTreeRunning(child);
}

async function raceTimeout(promise, timeout) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((resolve) => {
        timer = globalThis.setTimeout(() => resolve("timeout"), timeout);
      }),
    ]);
  } finally {
    if (timer !== undefined) globalThis.clearTimeout(timer);
  }
}

function onceExit(child) {
  if (childFinished(child) || child.spawnError) {
    return Promise.resolve(child.exitCode ?? 1);
  }
  return new Promise((resolve) => {
    const onError = () => finish(1);
    const onExit = (code) => finish(code ?? 1);
    const finish = (code) => {
      child.off("error", onError);
      child.off("exit", onExit);
      resolve(code);
    };
    child.once("error", onError);
    child.once("exit", onExit);
  });
}

function childFinished(child) {
  return child.exitCode !== null || child.signalCode !== null;
}

async function findExecutable(...candidates) {
  for (const candidate of candidates.filter(Boolean)) {
    if (candidate.includes(path.sep)) {
      if (await executable(candidate)) return path.resolve(candidate);
      continue;
    }
    for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
      const resolved = path.join(directory, candidate);
      if (await executable(resolved)) return resolved;
    }
  }
  throw new Error(
    `missing executable; checked ${candidates.filter(Boolean).join(", ")}`,
  );
}

async function executable(file) {
  try {
    await fs.access(file, fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function samePath(left, right) {
  const [canonicalLeft, canonicalRight] = await Promise.all([
    fs.realpath(left),
    fs.realpath(right),
  ]);
  return canonicalLeft === canonicalRight;
}

async function timed(action) {
  const started = performance.now();
  const value = await action();
  return { value, elapsedMs: rounded(performance.now() - started) };
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

function rounded(value) {
  return Math.round(value * 100) / 100;
}

function linuxClockTicks() {
  clockTicks ??= Number(
    execFileSync("getconf", ["CLK_TCK"], { encoding: "utf8" }).trim(),
  );
  assert.ok(Number.isFinite(clockTicks) && clockTicks > 0);
  return clockTicks;
}

async function findApplicationProcess(binary, expectedConfigHome) {
  const expectedBinary = await fs.realpath(binary);
  const expectedEnvironment = `XDG_CONFIG_HOME=${expectedConfigHome}`;
  const matches = [];
  for (const entry of await fs.readdir("/proc", { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^[1-9][0-9]*$/.test(entry.name)) continue;
    const pid = Number(entry.name);
    const executablePath = await fs
      .realpath(`/proc/${pid}/exe`)
      .catch(() => undefined);
    if (executablePath !== expectedBinary) continue;
    const environment = await fs
      .readFile(`/proc/${pid}/environ`)
      .catch(() => undefined);
    if (
      environment &&
      environment.toString("utf8").split("\0").includes(expectedEnvironment)
    ) {
      matches.push(pid);
    }
  }
  assert.equal(
    matches.length,
    1,
    `expected one isolated Mendimaru process, found ${matches.join(", ") || "none"}`,
  );
  return matches[0];
}

async function linuxProcessSnapshot(rootPid) {
  const records = [];
  for (const entry of await fs.readdir("/proc", { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^[1-9][0-9]*$/.test(entry.name)) continue;
    const pid = Number(entry.name);
    const stat = await fs
      .readFile(`/proc/${pid}/stat`, "utf8")
      .catch(() => undefined);
    if (!stat) continue;
    const closingParenthesis = stat.lastIndexOf(")");
    if (closingParenthesis < 0) continue;
    const fields = stat
      .slice(closingParenthesis + 2)
      .trim()
      .split(/\s+/);
    if (fields.length < 22) continue;
    records.push({
      pid,
      parentPid: Number(fields[1]),
      cpuTicks: Number(fields[11]) + Number(fields[12]),
    });
  }

  const processIds = new Set([rootPid]);
  let added;
  do {
    added = false;
    for (const record of records) {
      if (processIds.has(record.parentPid) && !processIds.has(record.pid)) {
        processIds.add(record.pid);
        added = true;
      }
    }
  } while (added);

  let cpuTicks = 0;
  let privateMemoryBytes = 0;
  let workingSetBytes = 0;
  let sampled = 0;
  for (const pid of processIds) {
    const record = records.find((candidate) => candidate.pid === pid);
    if (!record) continue;
    const memory = await linuxProcessMemory(pid);
    if (!memory) continue;
    cpuTicks += record.cpuTicks;
    privateMemoryBytes += memory.privateMemoryBytes;
    workingSetBytes += memory.workingSetBytes;
    sampled += 1;
  }
  assert.ok(
    sampled > 0,
    `could not sample process tree rooted at PID ${rootPid}`,
  );
  return {
    cpuSeconds: cpuTicks / linuxClockTicks(),
    privateMemoryBytes,
    workingSetBytes,
    processCount: sampled,
  };
}

async function linuxProcessMemory(pid) {
  const status = await fs
    .readFile(`/proc/${pid}/status`, "utf8")
    .catch(() => undefined);
  if (!status) return undefined;
  const workingSetBytes = kilobytesFromProcStatus(status, "VmRSS");
  const smaps = await fs
    .readFile(`/proc/${pid}/smaps_rollup`, "utf8")
    .catch(() => undefined);
  const privateMemoryBytes = smaps
    ? ["Private_Clean", "Private_Dirty", "Private_Hugetlb"].reduce(
        (total, key) => total + kilobytesFromProcStatus(smaps, key),
        0,
      )
    : workingSetBytes;
  return { privateMemoryBytes, workingSetBytes };
}

function kilobytesFromProcStatus(content, key) {
  const match = content.match(new RegExp(`^${key}:\\s+([0-9]+)\\s+kB$`, "m"));
  return match ? Number(match[1]) * 1024 : 0;
}

async function retainFailureScreenshot() {
  if (!temporary) return;
  if (sessionId) {
    try {
      const screenshot = await command("GET", "/screenshot");
      await fs.writeFile(
        path.join(temporary, "tauri-e2e-failure.png"),
        Buffer.from(screenshot, "base64"),
      );
      await fs.writeFile(
        path.join(artifactDirectory, "linux-tauri-failure.png"),
        Buffer.from(screenshot, "base64"),
      );
    } catch {
      // The original failure remains the useful result if the window already exited.
    }
  }
  for (const child of processes) {
    if (child.output) {
      await fs.writeFile(
        path.join(temporary, `${child.label}.log`),
        child.output,
      );
      process.stderr.write(`\n[${child.label}]\n${child.output}\n`);
    }
  }
}
