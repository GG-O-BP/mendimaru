// Installed Linux + WinBoat integration gate for #149. The operator owns a
// disposable, snapshot-verified Studio F5 session; no fixture is substituted.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import path from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import {
  assertPreserved,
  ownerStatus,
  processIdentity,
  rdpProcesses,
} from "./test-browser-winboat-live.mjs";
const exec = promisify(execFile);
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function validateReadSuite(suite) {
  assert.equal(suite.schemaVersion, "1.0.0");
  assert(suite.tests?.length >= 2, "provide at least two app assertions");
  for (const test of suite.tests) {
    assert.equal(
      test.concurrency?.resource,
      "app-read",
      "gate participants must declare stable app reads",
    );
    assert(
      test.steps?.some((step) =>
        ["expectVisible", "expectText", "expectValue"].includes(step.action),
      ),
      "each test needs an app assertion",
    );
  }
}

export function assertRun(result, { passed = true, parallel = false } = {}) {
  assert.equal(result.data?.outcome, passed ? "passed" : "failed");
  if (!passed) return;
  assert(result.data.passed > 0 && result.data.failed === 0);
  assert.equal(result.data.browserParity, "unmodified");
  assert.equal(result.data.environment?.comparable, true);
  assert.equal(result.data.concurrency?.sessionRole, "participant");
  if (parallel) assert(result.data.concurrency.maxObservedParallel >= 2);
  assert(result.data.corrections?.every((entry) => !entry.applied));
}

export async function runSharedGate() {
  assert.equal(process.platform, "linux");
  assert.equal(process.env.MENDIMARU_E2E_ALLOW_MUTATION, "1");
  assert(
    process.env.MENDIMARU_E2E_DISPOSABLE_SNAPSHOT,
    "verify a restorable disposable snapshot first",
  );
  assert(!process.env.MENDIMARU_E2E_VERSION);
  for (const name of ["MENDIMARU_BROWSER_RUNNER_PATH", "MENDIMARU_NODE_BINARY"])
    assert(!process.env[name], "installed gate forbids runner/Node overrides");
  const required = (name) => {
    assert(process.env[name], `set ${name}`);
    return process.env[name];
  };
  const binary = required("MENDIMARU_E2E_BINARY");
  const runtime = required("MENDIMARU_E2E_RUNTIME_SESSION_ID");
  const keeper = required("MENDIMARU_E2E_KEEPER_PID");
  const suitePath = required("MENDIMARU_E2E_BROWSER_SUITE");
  const marker = required("MENDIMARU_E2E_BUILD_MARKER");
  const cache = required("MENDIMARU_CACHE_DIR");
  const config = JSON.parse(
    await fs.readFile(
      path.join(required("MENDIMARU_CONFIG_DIR"), "config.json"),
      "utf8",
    ),
  );
  assert(
    config.containerName.startsWith("Mendimaru149"),
    "reserve an isolated #149 VM",
  );
  assert(!binary.includes("/target/"), "use an installed package binary");
  const suite = JSON.parse(await fs.readFile(suitePath, "utf8"));
  validateReadSuite(suite);
  const evidence = required("MENDIMARU_E2E_SHARED_REPORT");
  const children = new Set();
  const completions = new Map();
  const calls = [];
  const start = (...args) => {
    const child = spawn(
      binary,
      [...args, "--json", "--timeout-seconds", "180"],
      { stdio: ["ignore", "pipe", "pipe"] },
    );
    children.add(child);
    let stdout = "",
      stderr = "";
    for (const [stream, append] of [
      [
        child.stdout,
        (s) => {
          stdout += s;
        },
      ],
      [
        child.stderr,
        (s) => {
          stderr += s;
        },
      ],
    ]) {
      stream.on("data", (bytes) => {
        append(bytes.toString());
        if (stdout.length + stderr.length > 16 * 1024 * 1024)
          child.kill("SIGTERM");
      });
    }
    const timer = setTimeout(() => child.kill("SIGKILL"), 195_000);
    const done = new Promise((resolve, reject) => {
      child.once("error", (error) => {
        clearTimeout(timer);
        children.delete(child);
        reject(error);
      });
      child.once("close", (code, signal) => {
        clearTimeout(timer);
        children.delete(child);
        let envelope;
        for (const text of [stdout, stderr]) {
          try {
            envelope = JSON.parse(text);
            break;
          } catch {
            /* private raw logs never enter evidence */
          }
        }
        resolve({ code, signal, ...(envelope ?? {}) });
      });
    });
    completions.set(child, done);
    return { child, done };
  };
  const cli = async (...args) => start(...args).done;
  let shared;
  const report = {
    issue: 149,
    startedAt: new Date().toISOString(),
    binarySha256: digest(await fs.readFile(binary)),
    suiteSha256: digest(await fs.readFile(suitePath)),
    steps: calls,
    outcome: "failed",
  };
  const status = await cli("runtime", "status", "--session-id", runtime);
  assert.equal(status.data?.mode, "studio-run-locally");
  assert.equal(status.data.state, "ready");
  assert(status.data.studioSessionId, "real keeper-linked Studio F5 required");
  const snapshot = async () => {
    const [container, compose, studio, owner, rdp] = await Promise.all([
      exec(
        config.containerRuntime,
        [
          "inspect",
          "--format",
          '{"id":{{json .Id}},"state":{{json .State.Status}},"ports":{{json .NetworkSettings.Ports}}}',
          config.containerName,
        ],
        { timeout: 5000 },
      ),
      fs.readFile(config.composeFile),
      ownerStatus(cache, status.data.studioSessionId),
      processIdentity(keeper),
      rdpProcesses(),
    ]);
    return {
      container: JSON.parse(container.stdout),
      compose: digest(compose),
      studio,
      keeper: owner,
      rdp,
    };
  };
  const baseline = await snapshot();
  report.baseline = baseline;
  const preserved = async (name) => {
    assertPreserved(baseline, await snapshot());
    calls.push({ name, preserved: true });
  };
  const sessionStatus = async () =>
    (await cli("browser", "session", "status", "--shared-session-id", shared))
      .data;
  const until = async (condition) => {
    const deadline = Date.now() + 20_000;
    while (!(await condition())) {
      assert(Date.now() < deadline, "shared gate barrier timed out");
      await delay(100);
    }
  };
  const run = (file = suitePath, workers = "2", extra = []) =>
    start(
      "browser",
      "test",
      "--shared-session-id",
      shared,
      "--suite-path",
      file,
      "--asset-mirror",
      "off",
      "--workers",
      workers,
      "--fail-on-console-error",
      "--fail-on-network-failure",
      "--assertion-timeout-ms",
      "30000",
      "--retention-runs",
      "2",
      ...extra,
    );
  const temporary = await fs.mkdtemp(
    path.join(path.dirname(evidence), "suites-"),
  );
  let failure;
  try {
    const prepared = await cli(
      "browser",
      "session",
      "prepare",
      "--runtime-session-id",
      runtime,
      "--build-marker",
      marker,
      "--owns-runtime",
      "--finalize-policy",
      "stop",
    );
    shared = prepared.data?.sessionId;
    assert(shared && prepared.data.preparation?.comparable);
    report.preparationId = prepared.data.preparation.preparationId;
    const first = run(),
      second = run();
    await until(async () => (await sessionStatus()).liveParticipants >= 2);
    const busy = await cli("runtime", "stop", "--session-id", runtime);
    assert.equal(busy.error?.code, "precondition_failed");
    assert.equal(busy.error.retryable, true);
    const writerSuite = path.join(temporary, "writer.browser.json");
    const writer = structuredClone(suite);
    writer.tests = [writer.tests[0]];
    writer.tests[0].concurrency = { resource: "data-write" };
    await fs.writeFile(writerSuite, JSON.stringify(writer));
    const writeBusy = await run(writerSuite, "1").done;
    assert.equal(writeBusy.error?.code, "precondition_failed");
    assert.equal(writeBusy.error.retryable, true);
    const uiBusy = await cli(
      "ui",
      "release",
      "--session-id",
      status.data.studioSessionId,
      "--timeout-ms",
      "1000",
    );
    assert.equal(uiBusy.error?.code, "precondition_failed");
    assert.equal(uiBusy.error.retryable, true);
    const outcomes = await Promise.all([first.done, second.done]);
    outcomes.forEach((value) => {
      assertRun(value, { parallel: true });
      assert.equal(value.data.environment.preparationId, report.preparationId);
    });
    calls.push({
      name: "two-processes-two-lanes",
      passed: outcomes.map((r) => r.data.passed),
      preparationIds: outcomes.map((r) => r.data.environment.preparationId),
      peakParticipants: 2,
      lifecycleRefused: true,
      conflictingWriterRefused: true,
      uiMutationRefused: true,
    });
    const retained = await Promise.all(
      outcomes.map((result) =>
        cli("browser", "artifacts", "--session-id", result.data.sessionId),
      ),
    );
    retained.forEach((result) => assert.equal(result.ok, true));
    const retainedRuns = (
      await fs.readdir(path.join(cache, "browser-tests", "runs"))
    ).filter((name) => /^session_[a-f0-9]{32}$/.test(name));
    assert.equal(retainedRuns.length, 2);
    calls.push({ name: "concurrent-commit-prune-and-read", retainedRuns: 2 });
    await preserved("concurrent-readers");

    const failing = structuredClone(suite);
    failing.tests = [
      {
        name: "intentional bounded worker failure",
        concurrency: { resource: "app-read" },
        steps: [
          { action: "goto", path: "/" },
          {
            action: "expectVisible",
            locator: {
              by: "testId",
              value: "mendimaru-intentionally-absent-149",
            },
          },
        ],
      },
    ];
    const failedSuite = path.join(temporary, "failure.browser.json");
    await fs.writeFile(failedSuite, JSON.stringify(failing));
    for (const mode of ["failure", "SIGTERM", "SIGKILL"]) {
      const survivor = run();
      const victim = run(
        failedSuite,
        "1",
        mode === "failure" ? ["--worker-timeout-ms", "1500"] : [],
      );
      await until(async () => (await sessionStatus()).liveParticipants >= 2);
      if (mode !== "failure") victim.child.kill(mode);
      const victimResult = await victim.done;
      if (mode === "failure") assertRun(victimResult, { passed: false });
      assertRun(await survivor.done, { parallel: true });
      await until(async () => (await sessionStatus()).liveParticipants === 0);
      await preserved(`isolated-${mode}`);
    }
    // A finalizer crash must not strand ownership. Its replacement drains
    // the same live participant before the sole authorized cleanup below.
    const survivor = run();
    await until(async () => (await sessionStatus()).liveParticipants > 0);
    const finalizer = start(
      "browser",
      "session",
      "finalize",
      "--shared-session-id",
      shared,
      "--timeout-ms",
      "60000",
    );
    await until(async () => (await sessionStatus()).state === "finalizing");
    finalizer.child.kill("SIGKILL");
    await finalizer.done;
    const refused = await run().done;
    assert.equal(refused.error?.code, "precondition_failed");
    assertRun(await survivor.done, { parallel: true });
    await preserved("finalizer-crash-and-late-attach-refusal");
    const finalized = await cli(
      "browser",
      "session",
      "finalize",
      "--shared-session-id",
      shared,
    );
    assert.equal(finalized.data?.cleanup, "runtime-stopped");
    assert.equal((await sessionStatus()).liveParticipants, 0);
    const afterStop = await exec(config.containerRuntime, [
      "inspect",
      "--format",
      "{{.Id}}",
      config.containerName,
    ]);
    const duplicate = await cli(
      "browser",
      "session",
      "finalize",
      "--shared-session-id",
      shared,
    );
    assert.equal(duplicate.data?.alreadyFinalized, true);
    const afterDuplicate = await exec(config.containerRuntime, [
      "inspect",
      "--format",
      "{{.Id}}",
      config.containerName,
    ]);
    assert.equal(afterDuplicate.stdout, afterStop.stdout);
    calls.push({
      name: "owner-cleanup-after-last-participant",
      cleanup: finalized.data.cleanup,
      duplicateNoRecreation: true,
    });
    report.outcome = "passed";
  } catch (error) {
    failure = error;
  } finally {
    const remaining = [...children];
    for (const child of remaining) child.kill("SIGTERM");
    const killTimer = setTimeout(() => {
      for (const child of remaining) {
        if (children.has(child)) child.kill("SIGKILL");
      }
    }, 5000);
    await Promise.allSettled(remaining.map((child) => completions.get(child)));
    clearTimeout(killTimer);
    report.finishedAt = new Date().toISOString();
    await fs.writeFile(evidence, `${JSON.stringify(report, null, 2)}\n`);
    await fs.rm(temporary, { recursive: true, force: true });
  }
  if (failure) throw failure;
  process.stdout.write(`${JSON.stringify(report)}\n`);
}
if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  runSharedGate().catch((error) => {
    process.stderr.write(`Shared WinBoat gate failed: ${error.message}\n`);
    process.exitCode = 1;
  });
}
