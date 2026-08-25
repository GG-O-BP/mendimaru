import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import readline from "node:readline";
import { setTimeout } from "node:timers";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";
import {
  DEFAULT_ARTIFACT_SAFETY_LIMITS,
  inspectZipArchive,
} from "./browser-artifact-safety.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const runner = path.join(repository, "scripts/browser-runner.mjs");
const fixtureServer = path.join(repository, "tests/browser/fixture-server.mjs");
const password = 'canary P@"ss&word</trace+har-2026';
const username = "fixture-user";
const MAX_COMPRESSIBLE_RUNNER_RSS_BYTES = 512 * 1024 * 1024;
const temporary = await fs.mkdtemp(
  path.join(os.tmpdir(), "mendimaru-browser-e2e-"),
);
const storageStatePath = path.join(temporary, "storage-state.json");
await fs.writeFile(
  storageStatePath,
  `${JSON.stringify({
    cookies: [
      {
        name: "fixture_auth",
        value: password,
        domain: "127.0.0.1",
        path: "/",
        expires: -1,
        httpOnly: true,
        secure: false,
        sameSite: "Lax",
      },
    ],
    origins: [],
  })}\n`,
  { mode: 0o600 },
);
const server = spawn(process.execPath, [fixtureServer], {
  cwd: repository,
  stdio: ["ignore", "pipe", "pipe"],
});

try {
  const { port } = await firstJsonLine(server.stdout);
  const baseUrl = `http://127.0.0.1:${port}/`;
  const doctor = await invokeRunner("doctor");
  assert.equal(
    doctor.ready,
    true,
    "Chromium must be launchable for strict E2E",
  );
  assert.equal(doctor.nodeSupported, true);
  assert.equal(doctor.minimumNodeVersion, "22.22.2");
  assert.match(doctor.playwrightVersion, /^\d+\.\d+\.\d+$/);
  assert.match(doctor.chromium.version, /^\d+/);

  const isolatedBrowsers = path.join(temporary, "missing-browsers");
  await fs.mkdir(isolatedBrowsers, { mode: 0o700 });
  const missingDoctor = await invokeRunner("doctor", undefined, {
    PLAYWRIGHT_BROWSERS_PATH: isolatedBrowsers,
  });
  assert.equal(missingDoctor.ready, false);
  assert.equal(missingDoctor.chromium.installed, false);
  assert.equal(missingDoctor.chromium.launchable, false);

  const missingOutput = path.join(temporary, "missing-browser-run");
  await fs.mkdir(missingOutput, { mode: 0o700 });
  const missingBrowser = await invokeRunnerFailure(
    "run",
    {
      schemaVersion: "3.0.0",
      sessionId: `session_${randomBytes(16).toString("hex")}`,
      baseUrl,
      outputDirectory: missingOutput,
      runtimeContext: {
        hostPlatform: "linux",
        studioPlatform: "windows",
        runtimePlatform: "linux",
        backend: "linux-winboat",
        runtimeMode: "portable",
        runtimeVersion: "11.12.2",
      },
      policy: {
        navigationTimeoutMilliseconds: 15_000,
        actionTimeoutMilliseconds: 5_000,
        assertionTimeoutMilliseconds: 1_000,
        failOnConsoleError: true,
        failOnNetworkFailure: true,
        recordVideo: false,
        recordHar: false,
        maxArtifactBytes: 128 * 1024 * 1024,
        retentionRuns: 20,
      },
      suite: JSON.parse(
        await fs.readFile(
          path.join(repository, "tests/browser/smoke.browser.json"),
          "utf8",
        ),
      ),
    },
    { PLAYWRIGHT_BROWSERS_PATH: isolatedBrowsers },
  );
  assert.equal(missingBrowser.error.code, "chromium_unavailable");
  assert.deepEqual(await fs.readdir(missingOutput), []);

  const portable = await runSuite({
    baseUrl,
    mode: "portable",
    name: "portable",
    suite: "smoke.browser.json",
  });
  assert.equal(portable.result.outcome, "passed");
  assert.equal(portable.result.passed, 1);
  assert.equal(portable.result.failed, 0);
  await verifyManifest(portable.directory, "portable", "linux");
  await verifyNoSecret(portable.directory);

  const storageAuth = await runSuite({
    baseUrl,
    mode: "portable",
    name: "storage-auth",
    suite: "storage-auth.browser.json",
  });
  assert.equal(storageAuth.result.outcome, "passed");
  await verifyNoSecret(storageAuth.directory);

  const winboat = await runSuite({
    baseUrl,
    mode: "studio-run-locally",
    name: "winboat",
    policy: { recordHar: true, recordVideo: true },
    runtimePlatform: "windows",
    suite: "smoke.browser.json",
  });
  assert.equal(winboat.result.outcome, "passed");
  assert(winboat.result.files.some(({ path: name }) => name.endsWith(".har")));
  assert(winboat.result.files.some(({ path: name }) => name.endsWith(".webm")));
  await verifyManifest(winboat.directory, "studio-run-locally", "windows");
  await verifyNoSecret(winboat.directory);

  const failure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "failure",
    suite: "failure.browser.json",
  });
  assert.equal(failure.result.outcome, "failed");
  assert.equal(failure.result.failed, 1);
  for (const suffix of [
    "-failure.png",
    "-dom.html",
    "-accessibility.json",
    "-trace.zip",
  ]) {
    assert(
      failure.result.files.some(({ path: name }) => name.endsWith(suffix)),
      `missing strict failure artifact ${suffix}`,
    );
  }
  await verifyNoSecret(failure.directory);

  const pageFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "page-error",
    suite: "page-error.browser.json",
  });
  assert.equal(pageFailure.result.outcome, "failed");
  const pageErrors = JSON.parse(
    await fs.readFile(
      path.join(pageFailure.directory, "page-errors.json"),
      "utf8",
    ),
  );
  assert(
    pageErrors.entries.some(({ message }) => /uncaught failure/.test(message)),
  );
  await verifyNoSecret(pageFailure.directory);

  const timeoutFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "navigation-timeout",
    policy: { navigationTimeoutMilliseconds: 100 },
    suite: "timeout.browser.json",
  });
  assert.equal(timeoutFailure.result.outcome, "failed");
  assert.match(timeoutFailure.result.tests[0].failure, /timeout/i);

  const crossOriginFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "cross-origin",
    suite: "cross-origin.browser.json",
  });
  assert.equal(crossOriginFailure.result.outcome, "failed");
  assert.match(
    crossOriginFailure.result.tests[0].failure,
    /configured origin/i,
  );

  const consoleFailure = await runSuite({
    baseUrl,
    mode: "portable",
    name: "console-strict",
    policy: { failOnConsoleError: true },
    suite: "console.browser.json",
  });
  assert.equal(consoleFailure.result.outcome, "failed");
  const consoleReport = JSON.parse(
    await fs.readFile(
      path.join(consoleFailure.directory, "console.json"),
      "utf8",
    ),
  );
  assert(consoleReport.entries.some(({ type }) => type === "error"));

  const consoleAllowed = await runSuite({
    baseUrl,
    mode: "portable",
    name: "console-allowed",
    policy: { failOnConsoleError: false },
    suite: "console.browser.json",
  });
  assert.equal(consoleAllowed.result.outcome, "passed");

  const networkFailure = await runSuite({
    baseUrl,
    mode: "studio-run-locally",
    name: "network-strict",
    policy: { failOnNetworkFailure: true },
    runtimePlatform: "windows",
    suite: "network.browser.json",
  });
  assert.equal(networkFailure.result.outcome, "failed");
  const networkReport = JSON.parse(
    await fs.readFile(
      path.join(networkFailure.directory, "network-failures.json"),
      "utf8",
    ),
  );
  assert(networkReport.entries.some(({ status }) => status === 503));

  await runCompressibleTraceRejection(baseUrl);
  const recovery = await runSuite({
    baseUrl,
    mode: "portable",
    name: "post-limit-recovery",
    suite: "smoke.browser.json",
  });
  assert.equal(recovery.result.outcome, "passed");
  await verifyNoSecret(recovery.directory);

  const successSummary = JSON.parse(
    await fs.readFile(path.join(portable.directory, "summary.json"), "utf8"),
  );
  assert.equal(successSummary.schemaVersion, "3.0.0");
  assert.equal(successSummary.tests[0].completedSteps, 10);
  const html = await fs.readFile(
    path.join(portable.directory, "report.html"),
    "utf8",
  );
  assert.match(html, /Mendix fixture smoke/);
  assert.match(html, /Outcome: passed/);

  process.stdout.write(
    "browser E2E: 13 scenarios passed (Portable, WinBoat metadata, env/storage auth, assertion/page/navigation/origin failures, console/network policy, missing Chromium, video/HAR, bounded malicious trace, recovery, secret scan)\n",
  );
} finally {
  server.kill("SIGTERM");
  await Promise.race([onceExit(server), delay(2_000)]);
  if (server.exitCode === null) server.kill("SIGKILL");
  await fs.rm(temporary, { recursive: true, force: true });
}

async function runCompressibleTraceRejection(baseUrl) {
  const directory = path.join(temporary, "compressible-trace");
  await fs.mkdir(directory, { mode: 0o700 });
  const request = {
    schemaVersion: "3.0.0",
    sessionId: `session_${randomBytes(16).toString("hex")}`,
    baseUrl,
    outputDirectory: directory,
    runtimeContext: {
      hostPlatform: "linux",
      studioPlatform: "windows",
      runtimePlatform: "linux",
      backend: "linux-winboat",
      runtimeMode: "portable",
      runtimeVersion: "11.12.2",
    },
    policy: {
      navigationTimeoutMilliseconds: 15_000,
      actionTimeoutMilliseconds: 10_000,
      assertionTimeoutMilliseconds: 10_000,
      failOnConsoleError: true,
      failOnNetworkFailure: true,
      recordVideo: false,
      recordHar: false,
      maxArtifactBytes: 128 * 1024 * 1024,
      retentionRuns: 20,
    },
    suite: JSON.parse(
      await fs.readFile(
        path.join(repository, "tests/browser/compressible-trace.browser.json"),
        "utf8",
      ),
    ),
  };
  const { code, peakResidentBytes, stderr, stdout } = await invokeRunnerRaw(
    "run",
    request,
    {},
    true,
  );
  assert.equal(stderr, "", `browser runner wrote stderr: ${stderr}`);
  assert.equal(stdout.includes(password), false);
  const envelope = JSON.parse(stdout);
  const entries = await fs.readdir(directory);
  const trace = entries.find((name) => name.endsWith("-trace.zip"));
  assert.ok(trace, "malicious response must produce a trace fixture");
  const traceBytes = await fs.readFile(path.join(directory, trace));
  const maximumTraceMemberBytes = Math.max(
    0,
    ...[
      ...inspectZipArchive(traceBytes, {
        ...DEFAULT_ARTIFACT_SAFETY_LIMITS,
        maximumZipCompressionRatio: 1_000_000,
        maximumZipEntryBytes: 1024 * 1024 * 1024,
        maximumZipTotalBytes: 2 * 1024 * 1024 * 1024,
      }).values(),
    ].map(({ uncompressedSize }) => uncompressedSize),
  );
  assert.ok(
    maximumTraceMemberBytes > 64 * 1024 * 1024,
    `trace member ${maximumTraceMemberBytes} must exceed the safety limit`,
  );
  assert.equal(
    code,
    1,
    `ZIP bomb fixture unexpectedly succeeded with maximum member ${maximumTraceMemberBytes}: ${stdout}`,
  );
  assert.equal(envelope.ok, false);
  assert.equal(envelope.error.code, "artifact_limit_exceeded");
  assert.ok(
    peakResidentBytes > 0 &&
      peakResidentBytes < MAX_COMPRESSIBLE_RUNNER_RSS_BYTES,
    `bounded ${maximumTraceMemberBytes}-byte trace runner RSS ${peakResidentBytes} must remain below ${MAX_COMPRESSIBLE_RUNNER_RSS_BYTES}`,
  );
  assert.ok(
    traceBytes.length < 8 * 1024 * 1024,
    "malicious trace must have a small compressed representation",
  );
  assert.equal(entries.includes("artifact-manifest.json"), false);
  assert.equal(
    entries.some((name) => name.endsWith(".redact")),
    false,
  );
  process.stdout.write(
    `browser artifact security: compressed=${traceBytes.length} bytes, maximumMember=${maximumTraceMemberBytes} bytes, runnerPeakRss=${peakResidentBytes} bytes\n`,
  );
}

async function runSuite({
  baseUrl,
  mode,
  name,
  policy: policyOverrides = {},
  runtimePlatform = "linux",
  suite,
}) {
  const directory = path.join(temporary, name);
  await fs.mkdir(directory, { mode: 0o700 });
  const sessionId = `session_${randomBytes(16).toString("hex")}`;
  const policy = {
    navigationTimeoutMilliseconds: 15_000,
    actionTimeoutMilliseconds: 5_000,
    assertionTimeoutMilliseconds: 1_000,
    failOnConsoleError: true,
    failOnNetworkFailure: true,
    recordVideo: false,
    recordHar: false,
    maxArtifactBytes: 128 * 1024 * 1024,
    retentionRuns: 20,
    ...policyOverrides,
  };
  const request = {
    schemaVersion: "3.0.0",
    sessionId,
    baseUrl,
    outputDirectory: directory,
    runtimeContext: {
      hostPlatform: "linux",
      studioPlatform: "windows",
      runtimePlatform,
      backend: "linux-winboat",
      runtimeMode: mode,
      ...(mode === "studio-run-locally" ? { studioVersion: "11.12.2" } : {}),
      runtimeVersion: "11.12.2",
    },
    policy,
    suite: JSON.parse(
      await fs.readFile(path.join(repository, "tests/browser", suite), "utf8"),
    ),
  };
  const result = await invokeRunner("run", request);
  assert.equal(result.sessionId, sessionId);
  assert.equal(result.schemaVersion, "3.0.0");
  const actualFiles = new Set(await fs.readdir(directory));
  assert.deepEqual(
    actualFiles,
    new Set(result.files.map(({ path: filename }) => filename)),
    "runner output and artifact inventory must match exactly",
  );
  return { directory, result };
}

async function invokeRunner(command, request, environment = {}) {
  const { code, stderr, stdout } = await invokeRunnerRaw(
    command,
    request,
    environment,
  );
  assert.equal(stderr, "", `browser runner wrote stderr: ${stderr}`);
  assert.equal(stdout.trim().split("\n").length, 1);
  const envelope = JSON.parse(stdout);
  assert.equal(code, 0, `browser runner failed: ${JSON.stringify(envelope)}`);
  assert.equal(envelope.ok, true);
  return envelope.data;
}

async function invokeRunnerFailure(command, request, environment = {}) {
  const { code, stderr, stdout } = await invokeRunnerRaw(
    command,
    request,
    environment,
  );
  assert.equal(stderr, "", `browser runner wrote stderr: ${stderr}`);
  assert.equal(stdout.trim().split("\n").length, 1);
  const envelope = JSON.parse(stdout);
  assert.equal(code, 1, `browser runner unexpectedly succeeded: ${stdout}`);
  assert.equal(envelope.ok, false);
  return envelope;
}

async function invokeRunnerRaw(
  command,
  request,
  environment,
  measureMemory = false,
) {
  const child = spawn(process.execPath, [runner, command], {
    cwd: repository,
    env: {
      ...process.env,
      MENDIMARU_TEST_PASSWORD: password,
      MENDIMARU_TEST_STORAGE_STATE: storageStatePath,
      MENDIMARU_TEST_USERNAME: username,
      ...environment,
    },
    stdio: [request ? "pipe" : "ignore", "pipe", "pipe"],
  });
  if (request) child.stdin.end(JSON.stringify(request));
  const [stdout, stderr, code, peakResidentBytes] = await Promise.all([
    collect(child.stdout),
    collect(child.stderr),
    onceExit(child),
    measureMemory ? peakLinuxResidentBytes(child) : Promise.resolve(null),
  ]);
  return { code, peakResidentBytes, stderr, stdout };
}

async function peakLinuxResidentBytes(child) {
  if (process.platform !== "linux") return 0;
  let peak = 0;
  while (child.exitCode === null) {
    const status = await fs
      .readFile(`/proc/${child.pid}/status`, "utf8")
      .catch(() => "");
    const match = /^VmRSS:\s+(\d+)\s+kB$/m.exec(status);
    if (match) peak = Math.max(peak, Number(match[1]) * 1024);
    if (child.exitCode === null) await delay(10);
  }
  return peak;
}

async function verifyManifest(directory, mode, runtimePlatform) {
  const manifest = JSON.parse(
    await fs.readFile(path.join(directory, "artifact-manifest.json"), "utf8"),
  );
  assert.equal(manifest.schemaVersion, "3.0.0");
  assert.equal(manifest.hostPlatform, "linux");
  assert.equal(manifest.studioPlatform, "windows");
  assert.equal(manifest.runtimePlatform, runtimePlatform);
  assert.equal(manifest.backend, "linux-winboat");
  assert.equal(manifest.runtimeMode, mode);
  assert.equal(manifest.runtimeVersion, "11.12.2");
  if (mode === "studio-run-locally") {
    assert.equal(manifest.studioVersion, "11.12.2");
  }
  assert.equal(manifest.browser.name, "chromium");
  assert.match(manifest.browser.version, /^\d+/);
  assert.match(manifest.playwrightVersion, /^\d+\.\d+\.\d+$/);
  assert(manifest.artifacts.length >= 5);
  for (const artifact of manifest.artifacts) {
    const bytes = await fs.readFile(path.join(directory, artifact.file));
    assert.equal(artifact.sizeBytes, bytes.length);
    assert.equal(
      artifact.sha256,
      createHash("sha256").update(bytes).digest("hex"),
    );
  }
}

async function verifyNoSecret(directory) {
  const needles = [
    Buffer.from(password),
    Buffer.from(encodeURIComponent(password)),
    Buffer.from(encodeURIComponent(password).replaceAll("%20", "+")),
    Buffer.from(
      encodeURIComponent(password).replace(/%[0-9A-F]{2}/g, (value) =>
        value.toLowerCase(),
      ),
    ),
    Buffer.from(
      encodeURIComponent(password)
        .replace(/%[0-9A-F]{2}/g, (value) => value.toLowerCase())
        .replaceAll("%20", "+"),
    ),
    Buffer.from(JSON.stringify(password).slice(1, -1)),
    Buffer.from(htmlEscape(password, false)),
    Buffer.from(htmlEscape(password, true)),
    Buffer.from(Buffer.from(password).toString("base64")),
    Buffer.from(
      Buffer.from(password)
        .toString("base64")
        .replaceAll("+", "-")
        .replaceAll("/", "_"),
    ),
    Buffer.from(Buffer.from(password).toString("base64url")),
  ];
  for (const name of await fs.readdir(directory)) {
    const bytes = await fs.readFile(path.join(directory, name));
    const archive = name.endsWith(".zip") ? unzipSync(bytes) : null;
    const payloads = archive ? Object.values(archive) : [bytes];
    if (archive) {
      for (const memberName of Object.keys(archive)) {
        for (const needle of needles) {
          assert.equal(
            Buffer.from(memberName).includes(needle),
            false,
            `secret leaked into ${name} member name`,
          );
        }
      }
    }
    for (const payload of payloads) {
      for (const needle of needles) {
        assert.equal(
          Buffer.from(payload).includes(needle),
          false,
          `secret leaked into ${name}`,
        );
      }
    }
  }
}

function htmlEscape(value, quote) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', quote ? "&quot;" : '"')
    .replaceAll("'", quote ? "&#39;" : "'");
}

async function firstJsonLine(stream) {
  const lines = readline.createInterface({ input: stream });
  for await (const line of lines) {
    lines.close();
    return JSON.parse(line);
  }
  throw new Error("fixture server exited before publishing its port");
}

async function collect(stream) {
  let value = "";
  stream.setEncoding("utf8");
  for await (const chunk of stream) value += chunk;
  return value;
}

function onceExit(child) {
  if (child.exitCode !== null) return Promise.resolve(child.exitCode);
  return new Promise((resolve) =>
    child.once("exit", (code) => resolve(code ?? 1)),
  );
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
