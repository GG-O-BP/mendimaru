import assert from "node:assert/strict";
import { promises as fs } from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";

const MARKER_NAME = ".mendimaru-e2e-root";
const MARKER_CONTENT = "mendimaru isolated native e2e\n";
const ENVIRONMENT_MODE_NAME = "performance-environment-mode";
const LARGE_PROJECT_COUNT = 250;
const LARGE_PROJECT_BYTES = 256 * 1024;

export async function createReleaseFixture(platform = process.platform) {
  if (!["linux", "win32"].includes(platform)) {
    throw new Error(`unsupported performance fixture platform: ${platform}`);
  }
  const root = await fs.realpath(
    await fs.mkdtemp(path.join(os.tmpdir(), "mendimaru-perf-")),
  );
  const bin = path.join(root, "bin");
  const cache = path.join(root, "cache");
  const configDirectory = path.join(root, "config");
  const smallWorkspace = path.join(root, "workspace-small");
  const largeWorkspace = path.join(root, "workspace-large");
  const webviewCache = path.join(root, "webview-cache");
  const webviewData = path.join(root, "webview-data");
  await Promise.all(
    [
      bin,
      cache,
      configDirectory,
      smallWorkspace,
      largeWorkspace,
      webviewCache,
      webviewData,
    ].map((directory) => fs.mkdir(directory, { recursive: true })),
  );
  await fs.writeFile(path.join(root, MARKER_NAME), MARKER_CONTENT, {
    mode: 0o600,
  });
  const environmentModePath = path.join(root, ENVIRONMENT_MODE_NAME);
  await fs.writeFile(environmentModePath, "normal\n", { mode: 0o600 });

  const smallBytes = await createProject(
    smallWorkspace,
    "Orders",
    "11.12.2",
    1024,
  );
  let largeBytes = 0;
  for (let offset = 0; offset < LARGE_PROJECT_COUNT; offset += 25) {
    const batch = [];
    for (
      let index = offset;
      index < Math.min(offset + 25, LARGE_PROJECT_COUNT);
      index += 1
    ) {
      batch.push(
        createProject(
          largeWorkspace,
          `Project${String(index + 1).padStart(3, "0")}`,
          index % 2 === 0 ? "11.12.2" : "10.24.9",
          LARGE_PROJECT_BYTES,
        ),
      );
    }
    largeBytes += (await Promise.all(batch)).reduce(
      (total, bytes) => total + bytes,
      0,
    );
  }

  const marketplace = http.createServer((request, response) => {
    if (!request.url?.startsWith("/link/studiopro")) {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("not found");
      return;
    }
    response.writeHead(200, {
      "cache-control": "no-store",
      "content-type": "text/html; charset=utf-8",
    });
    response.end(marketplaceHtml());
  });
  const marketplacePort = await listen(marketplace);

  let guest;
  let rdp;
  let guestPort;
  let rdpPort;
  if (platform === "linux") {
    guest = http.createServer(async (request, response) => {
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
              Source: "release performance fixture",
            },
          ]),
        );
        return;
      }
      response.statusCode = 404;
      response.end('{"error":"not-found"}');
    });
    guestPort = await listen(guest);
    rdp = net.createServer((socket) => socket.end());
    rdpPort = await listen(rdp);
  }

  const config =
    platform === "linux"
      ? await linuxConfig({
          bin,
          root,
          sharedDirectory: smallWorkspace,
          guestPort,
          rdpPort,
        })
      : windowsConfig(smallWorkspace);
  const configPath = path.join(configDirectory, "config.json");
  await fs.writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`, {
    mode: 0o600,
  });
  await fs.writeFile(
    path.join(cache, "studio-version-catalog.json"),
    `${JSON.stringify({
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
      fetchedAt: new Date().toISOString(),
    })}\n`,
    { mode: 0o600 },
  );

  let closed = false;
  return {
    root,
    bin,
    cache,
    configDirectory,
    smallWorkspace,
    largeWorkspace,
    webviewCache,
    webviewData,
    marketplaceUrl: `http://127.0.0.1:${marketplacePort}/link/studiopro`,
    workspaceTiers: {
      small: { projectCount: 1, totalBytes: smallBytes },
      large: { projectCount: LARGE_PROJECT_COUNT, totalBytes: largeBytes },
    },
    async clearWebviewCache() {
      await Promise.all(
        [webviewCache, webviewData].map(async (directory) => {
          await fs.rm(directory, {
            force: true,
            recursive: true,
            maxRetries: 20,
            retryDelay: 250,
          });
          await fs.mkdir(directory, { recursive: true });
        }),
      );
    },
    async setWorkspace(directory) {
      if (![smallWorkspace, largeWorkspace].includes(directory)) {
        throw new Error(
          `workspace is outside the performance fixture: ${directory}`,
        );
      }
      const next = { ...config, sharedDirectory: directory };
      const temporary = `${configPath}.tmp`;
      await fs.writeFile(temporary, `${JSON.stringify(next, null, 2)}\n`, {
        mode: 0o600,
      });
      await fs.rename(temporary, configPath);
      config.sharedDirectory = directory;
    },
    async setEnvironmentMode(mode) {
      if (!["normal", "slow", "timeout"].includes(mode)) {
        throw new Error(`unsupported environment performance mode: ${mode}`);
      }
      const temporary = `${environmentModePath}.tmp`;
      await fs.writeFile(temporary, `${mode}\n`, { mode: 0o600 });
      await fs.rename(temporary, environmentModePath);
    },
    async close() {
      if (closed) return;
      closed = true;
      await Promise.all([marketplace, guest, rdp].filter(Boolean).map(close));
      await removeFixtureRoot(root);
    },
  };
}

async function linuxConfig({ bin, root, sharedDirectory, guestPort, rdpPort }) {
  const runtime = path.join(bin, "docker");
  const freerdp = path.join(bin, "xfreerdp3");
  const winboat = path.join(bin, "winboat");
  const compose = path.join(root, "compose.yml");
  const inspect = [
    {
      State: { Status: "running" },
      Mounts: [
        { Source: sharedDirectory, Destination: "/shared" },
        { Source: "mendimaru-perf-storage", Destination: "/storage" },
      ],
      NetworkSettings: {
        Ports: {
          "7148/tcp": [{ HostIp: "127.0.0.1", HostPort: String(guestPort) }],
          "3389/tcp": [{ HostIp: "127.0.0.1", HostPort: String(rdpPort) }],
        },
      },
    },
  ];
  await writeExecutable(
    runtime,
    `#!${process.execPath}\nconst args = process.argv.slice(2);\nconst inspect = ${JSON.stringify(inspect)};\nif (args[0] === "info") console.log("29.7.2");\nelse if (args[0] === "inspect") console.log(JSON.stringify(inspect));\nelse if (args[0] === "port" && args[2] === "7148/tcp") console.log("127.0.0.1:${guestPort}");\nelse if (args[0] === "port" && args[2] === "3389/tcp") console.log("127.0.0.1:${rdpPort}");\nelse if (args[0] !== "compose") process.exitCode = 1;\n`,
  );
  await writeExecutable(
    freerdp,
    `#!${process.execPath}\nif (process.argv.includes("/version")) console.log("FreeRDP version 3.30.0");\nelse process.exitCode = 1;\n`,
  );
  await writeExecutable(winboat, `#!${process.execPath}\nprocess.exit(0);\n`);
  await fs.writeFile(
    compose,
    `services:\n  windows:\n    image: ghcr.io/dockur/windows:performance-fixture\n    container_name: MendimaruPerformance\n    volumes:\n      - ${JSON.stringify(`${sharedDirectory}:/shared`)}\n      - mendimaru-perf-storage:/storage\n    ports:\n      - 127.0.0.1:${guestPort}:7148\n      - 127.0.0.1:${rdpPort}:3389\nvolumes:\n  mendimaru-perf-storage: {}\n`,
  );
  return {
    languagePreference: "en-US",
    winboatSetupPending: false,
    winboatExecutable: winboat,
    composeFile: compose,
    containerRuntime: "docker",
    containerName: "MendimaruPerformance",
    apiUrl: `http://127.0.0.1:${guestPort}`,
    rdpHost: "127.0.0.1",
    rdpPort,
    sharedDirectory,
    windowsSharedDirectory: String.raw`\\host.lan\Data`,
    freerdpBinary: freerdp,
    mendixInstallRoot: String.raw`C:\Program Files\Mendix`,
    mendixDataRoot: String.raw`C:\ProgramData\Mendix`,
    windowsStudioPaths: [],
    startupTimeoutSeconds: 2,
  };
}

function windowsConfig(sharedDirectory) {
  const programFiles =
    process.env.ProgramW6432 ??
    process.env.ProgramFiles ??
    String.raw`C:\Program Files`;
  const programData = process.env.ProgramData ?? String.raw`C:\ProgramData`;
  return {
    languagePreference: "en-US",
    winboatSetupPending: false,
    winboatExecutable: "",
    composeFile: "",
    containerRuntime: "docker",
    containerName: "",
    apiUrl: "",
    rdpHost: "",
    rdpPort: 0,
    sharedDirectory,
    windowsSharedDirectory: "",
    freerdpBinary: "",
    mendixInstallRoot: path.join(programFiles, "Mendix"),
    mendixDataRoot: path.join(programData, "Mendix"),
    windowsStudioPaths: [],
    startupTimeoutSeconds: 180,
  };
}

async function createProject(workspace, name, version, payloadBytes) {
  const directory = path.join(workspace, name);
  await fs.mkdir(directory, { recursive: true });
  const payload = Buffer.alloc(payloadBytes, 0x4d);
  const settings = `${JSON.stringify({
    settingsParts: [
      { type: `Mendix.Core, Version=${version}.0, Culture=neutral` },
    ],
  })}\n`;
  await Promise.all([
    fs.writeFile(path.join(directory, `${name}.mpr`), payload),
    fs.writeFile(path.join(directory, "project-settings.user.json"), settings),
  ]);
  return payload.length + Buffer.byteLength(settings);
}

function marketplaceHtml() {
  return `<!doctype html>
<html lang="en">
  <body>
    <div class="widget-datagrid-content">
      <div class="widget-datagrid-grid-body table-content" role="rowgroup">
        <div class="tr" role="row">
          <div role="gridcell"><div><div><a class="mx-name-actionButton_VersionName1" href="#">11.13.0</a><span>Latest</span></div></div></div>
          <div role="gridcell"><span>July 28, 2026</span></div>
          <div role="gridcell"><a href="https://docs.mendix.com/releasenotes/studio-pro/11.13/#11130">Release Notes</a></div>
        </div>
        <div class="tr" role="row">
          <div role="gridcell"><div><div><a class="mx-name-actionButton_VersionName1" href="#">11.12.2</a><span>LTS</span></div></div></div>
          <div role="gridcell"><span>July 27, 2026</span></div>
          <div role="gridcell"><a href="https://docs.mendix.com/releasenotes/studio-pro/11.12/#11122">Release Notes</a></div>
        </div>
      </div>
    </div>
  </body>
</html>`;
}

async function writeExecutable(file, content) {
  await fs.writeFile(file, content, { mode: 0o700 });
  await fs.chmod(file, 0o700);
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert(address && typeof address !== "string");
  return address.port;
}

async function close(server) {
  if (!server.listening) return;
  await new Promise((resolve) => server.close(resolve));
}

async function removeFixtureRoot(root) {
  const [canonicalRoot, canonicalTemporary] = await Promise.all([
    fs.realpath(root),
    fs.realpath(os.tmpdir()),
  ]);
  const relative = path.relative(canonicalTemporary, canonicalRoot);
  const marker = await fs.readFile(
    path.join(canonicalRoot, MARKER_NAME),
    "utf8",
  );
  if (
    !relative ||
    relative.startsWith(`..${path.sep}`) ||
    path.dirname(canonicalRoot) !== canonicalTemporary ||
    !path.basename(canonicalRoot).startsWith("mendimaru-perf-") ||
    marker !== MARKER_CONTENT
  ) {
    throw new Error(`refusing to remove unsafe performance fixture: ${root}`);
  }
  await fs.rm(canonicalRoot, {
    recursive: true,
    maxRetries: 3,
    retryDelay: 250,
  });
}
