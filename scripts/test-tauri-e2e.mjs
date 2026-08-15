import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { spawn } from "node:child_process";
import { constants as fsConstants, promises as fs } from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
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
const processes = [];
const servers = [];
const serverSockets = new Map();
let sessionId;
let webdriverUrl;
let temporary;
let succeeded = false;

try {
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

  assert.match(
    await command("GET", "/title"),
    /^mendimaru — Mendix Studio Pro Manager$/,
  );
  assert.equal(await execute("return location.href;"), viteUrl);
  assert.equal(
    await execute("return Boolean(window.__TAURI_INTERNALS__);"),
    true,
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

  await waitFor(
    async () =>
      await execute(`
        return document.querySelector(".route-status")?.classList.contains("online")
          && document.body.innerText.includes("WinBoat online")
          && document.body.innerText.includes("11.12.2")
          && document.body.innerText.includes("11.13.0");
      `),
    20_000,
    "the fixture-backed native commands did not populate the online Studio view",
  );

  const routeMotion = await execute(`
    const marker = document.querySelector(".route-track i");
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
        const marker = document.querySelector(".route-status.online .route-track i");
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
          ".route-status.online .route-track i",
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
            target.matches(".route-status.online .route-track i");
          return animation.playState === "running" && !allowlisted;
        })
        .map((animation) => ({
          animationName: animation.effect?.target
            ? getComputedStyle(animation.effect.target).animationName
            : "",
          className: animation.effect?.target?.className ?? "",
        })),
      routeMarkerPresent: Boolean(document.querySelector(".route-track i")),
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
    "the idle native window may run only the online route indicator",
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

  await assertView("Control+2", "Projects", "Orders");
  await assertView(
    "Control+3",
    "Operation center",
    "No operations have been recorded",
  );
  await assertView("Control+4", "Settings", "Environment diagnostics");
  await assertView("Control+1", "Studio Pro", "11.12.2");

  await delay(1_750);
  assert.deepEqual(
    await execute("return window.__MENDIMARU_E2E_ERRORS__;"),
    [],
    "the real WebView reported an error during the application flow",
  );

  succeeded = true;
} catch (error) {
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
}

if (succeeded) {
  process.stdout.write(
    "Tauri E2E: real WebKit window passed (dev URL, IPC contract, sampled route/busy motion, bounded idle rendering, four-view navigation)\n",
  );
}

async function assertView(shortcut, heading, expectedText) {
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
  if (!response.ok || envelope.value?.error) {
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

  const api = http.createServer((request, response) => {
    response.setHeader("content-type", "application/json");
    if (request.url === "/health") {
      response.end('{"status":"ok"}');
      return;
    }
    if (request.url === "/apps") {
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

  return { bin, chrome, xdgCache, xdgConfig };
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

async function retainFailureScreenshot() {
  if (!temporary) return;
  if (sessionId) {
    try {
      const screenshot = await command("GET", "/screenshot");
      await fs.writeFile(
        path.join(temporary, "tauri-e2e-failure.png"),
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
