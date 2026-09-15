import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import Ajv from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const repository = fileURLToPath(new URL("../", import.meta.url));
const binary = process.argv[2];
assert.ok(binary, "pass the compiled Mendimaru binary");
const temporary = await fs.mkdtemp(path.join(os.tmpdir(), "mendimaru-doctor-"));
const node = await fs.realpath(process.execPath);
const runner = path.join(repository, "scripts/browser-runner.mjs");
const canary = "private-token-do-not-publish";
const privatePath = `/home/private-user/${canary}`;
const ajv = new Ajv({ allErrors: true, strict: true });
addFormats(ajv);
const schema = JSON.parse(
  await fs.readFile(
    new URL("../schemas/browser.schema.json", import.meta.url),
    "utf8",
  ),
);
ajv.addSchema(schema);
const validate = ajv.getSchema(`${schema.$id}#/$defs/doctor`);
let count = 0;

async function file(name, contents, mode = 0o600) {
  const filename = path.join(temporary, name);
  await fs.writeFile(filename, contents, { mode });
  return filename;
}

function assertPrivate(text) {
  for (const secret of [canary, privatePath, temporary, node, runner]) {
    assert.ok(!text.includes(secret), `diagnostic leaked ${secret}`);
  }
}

async function check(name, overrides, failures, extra = () => {}) {
  const cache = path.join(temporary, `cache-${name}`);
  const env = {
    ...process.env,
    MENDIMARU_CONFIG_DIR: path.join(temporary, "config"),
    MENDIMARU_CACHE_DIR: cache,
    MENDIMARU_NODE_BINARY: node,
    MENDIMARU_BROWSER_RUNNER_PATH: runner,
    MENDIMARU_TEST_TOKEN: canary,
    ...overrides,
  };
  for (const key of Object.keys(env)) if (env[key] === null) delete env[key];
  const started = Date.now();
  const output = spawnSync(binary, ["browser", "doctor", "--json"], {
    env,
    encoding: "utf8",
    timeout: 30_000,
    maxBuffer: 128 * 1024,
  });
  assert.ifError(output.error);
  assert.equal(
    output.status,
    failures.length ? 1 : 0,
    `${name}: ${output.stdout} ${output.stderr}`,
  );
  assert.equal(output.stderr, "", name);
  assert.equal(output.stdout.trim().split("\n").length, 1, name);
  assertPrivate(output.stdout);
  const envelope = JSON.parse(output.stdout);
  assert.equal(envelope.command, "browser.doctor");
  assert.equal(envelope.ok, true);
  const report = envelope.data;
  assert.ok(validate(report), `${name}: ${JSON.stringify(validate.errors)}`);
  assert.equal(report.ready, !failures.length, name);
  assert.equal(report.downloadPolicy, "explicit-only");
  assert.deepEqual(
    report.checks.map(({ id }) => id),
    ["runner", "node", "node_version", "js_dependencies", "chromium"],
  );
  assert.deepEqual(
    report.checks
      .filter(({ status }) => status === "failed")
      .map(({ id, code }) => [id, code]),
    failures,
    name,
  );
  for (const item of report.checks)
    assert.ok(item.message.length && item.action.length);
  if (failures.length && report.diagnostic.stored) {
    const diagnostic = path.join(
      env.MENDIMARU_CACHE_DIR,
      "browser-tests/doctor/latest.json",
    );
    const content = await fs.readFile(diagnostic, "utf8");
    assert.ok(content.length <= 16 * 1024);
    assertPrivate(content);
    assert.deepEqual(JSON.parse(content), report);
    assert.equal((await fs.stat(diagnostic)).mode & 0o777, 0o600);
    assert.equal((await fs.stat(path.dirname(diagnostic))).mode & 0o777, 0o700);
    assert.deepEqual(await fs.readdir(path.dirname(diagnostic)), [
      "latest.json",
    ]);
  }
  await extra(report, Date.now() - started);
  count += 1;
}

try {
  await check("healthy", {}, []);
  await check(
    "runner-missing",
    { MENDIMARU_BROWSER_RUNNER_PATH: path.join(temporary, "absent.mjs") },
    [["runner", "runner_missing"]],
    (report) => {
      assert.equal(report.checks[2].status, "passed");
      assert.equal(report.checks[3].status, "skipped");
    },
  );
  await check(
    "both-missing",
    {
      MENDIMARU_BROWSER_RUNNER_PATH: path.join(temporary, "absent.mjs"),
      MENDIMARU_NODE_BINARY: path.join(temporary, "absent-node"),
    },
    [
      ["runner", "runner_missing"],
      ["node", "node_missing"],
    ],
  );
  for (const [key, id] of [
    ["MENDIMARU_BROWSER_RUNNER_PATH", "runner"],
    ["MENDIMARU_NODE_BINARY", "node"],
  ]) {
    const link = path.join(temporary, `${id}-link`);
    await fs.symlink(id === "node" ? node : runner, link);
    for (const [label, value] of [
      ["relative", "relative-private-value"],
      ["symlink", link],
      ["directory", temporary],
    ]) {
      await check(`${id}-${label}`, { [key]: value }, [
        [id, "unsafe_override"],
      ]);
    }
  }
  const unreadable = await file("unreadable.mjs", "", 0o000);
  await check(
    "runner-unreadable",
    { MENDIMARU_BROWSER_RUNNER_PATH: unreadable },
    [["runner", "runner_unreadable"]],
  );
  await check(
    "node-path-missing",
    { PATH: temporary, MENDIMARU_NODE_BINARY: null },
    [["node", "node_missing"]],
  );
  const denied = await file("denied-node", "#!/bin/sh\nexit 0\n");
  await check("node-denied", { MENDIMARU_NODE_BINARY: denied }, [
    ["node", "node_spawn_denied"],
  ]);
  const invalidExecutable = await file(
    "invalid-node",
    "invalid executable",
    0o700,
  );
  await check(
    "node-spawn-failed",
    { MENDIMARU_NODE_BINARY: invalidExecutable },
    [["node", "node_spawn_failed"]],
  );
  for (const [label, version, cause] of [
    ["unsupported", "v22.22.1", "node_unsupported"],
    ["invalid", privatePath, "node_probe_failed"],
  ]) {
    const fake = await file(
      `node-${label}`,
      `#!/bin/sh\nprintf '%s\\n' '${version}'\n`,
      0o700,
    );
    await check(`node-${label}`, { MENDIMARU_NODE_BINARY: fake }, [
      ["node_version", cause],
    ]);
  }
  const hangingNode = await file(
    "hanging-node",
    "#!/bin/sh\nsleep 60\n",
    0o700,
  );
  await check(
    "node-timeout",
    { MENDIMARU_NODE_BINARY: hangingNode },
    [["node", "probe_timeout"]],
    (_report, elapsed) => assert.ok(elapsed < 6_000),
  );

  const isolatedRunner = await file(
    "isolated-runner.mjs",
    await fs.readFile(runner),
  );
  await check(
    "dependencies-missing",
    { MENDIMARU_BROWSER_RUNNER_PATH: isolatedRunner },
    [["js_dependencies", "js_dependencies_missing"]],
    (report) => {
      assert.equal(report.diagnostic.errorKind, "ERR_MODULE_NOT_FOUND");
      assert.equal(report.diagnostic.module, "@playwright/test");
      assert.equal(report.checks[4].status, "skipped");
    },
  );
  const moduleRunner = await file(
    "module-runner.mjs",
    `import "fflate"; // ${privatePath}\n`,
  );
  await check(
    "module-missing",
    { MENDIMARU_BROWSER_RUNNER_PATH: moduleRunner },
    [["js_dependencies", "js_dependencies_missing"]],
    (report) => assert.equal(report.diagnostic.module, "fflate"),
  );
  const syntaxRunner = await file(
    "syntax-runner.mjs",
    `const ${canary} = ; // ${privatePath}\n`,
  );
  await check(
    "syntax-error",
    { MENDIMARU_BROWSER_RUNNER_PATH: syntaxRunner },
    [["js_dependencies", "runner_failed"]],
    (report) => assert.equal(report.diagnostic.errorKind, "SyntaxError"),
  );
  const floodRunner = await file(
    "flood-runner.mjs",
    `process.stderr.write(${JSON.stringify(privatePath)}.repeat(100000)); process.stdout.write('x'.repeat(100000)); process.exitCode = 1;\n`,
  );
  await check(
    "output-flood",
    { MENDIMARU_BROWSER_RUNNER_PATH: floodRunner },
    [["js_dependencies", "runner_failed"]],
    (report) => {
      assert.equal(report.diagnostic.stderrTruncated, true);
      assert.equal(report.diagnostic.stdoutTruncated, true);
    },
  );
  const malformedRunner = await file(
    "malformed-runner.mjs",
    `console.log(${JSON.stringify(privatePath)});\n`,
  );
  await check(
    "invalid-response",
    { MENDIMARU_BROWSER_RUNNER_PATH: malformedRunner },
    [["js_dependencies", "runner_output_invalid"]],
  );
  // Reject private strings even in otherwise schema-shaped version fields,
  // including an unavailable browser that cannot establish a launch identity.
  for (const field of ["nodeVersion", "playwrightVersion", "chromium"]) {
    const data = {
      schemaVersion: "4.0.0",
      runnerVersion: "1.0.0",
      ready: false,
      nodeVersion: process.versions.node,
      minimumNodeVersion: "22.22.2",
      nodeSupported: true,
      playwrightVersion: "1.62.1",
      chromium: { installed: false, launchable: false },
      downloadPolicy: "explicit-only",
    };
    data[field] =
      field === "chromium"
        ? { ...data.chromium, version: privatePath }
        : privatePath;
    const privateRunner = await file(
      `private-${field}.mjs`,
      `console.log(${JSON.stringify(JSON.stringify({ ok: true, data }))});\n`,
    );
    await check(
      `private-${field}`,
      { MENDIMARU_BROWSER_RUNNER_PATH: privateRunner },
      [["js_dependencies", "runner_output_invalid"]],
    );
  }
  const hangingRunner = await file(
    "hanging-runner.mjs",
    "setInterval(() => {}, 1000);\n",
  );
  await check(
    "runner-timeout",
    { MENDIMARU_BROWSER_RUNNER_PATH: hangingRunner },
    [["js_dependencies", "probe_timeout"]],
    (_report, elapsed) => assert.ok(elapsed < 24_000),
  );

  const browsers = path.join(temporary, "browsers");
  await check(
    "chromium-missing",
    { PLAYWRIGHT_BROWSERS_PATH: browsers },
    [["chromium", "chromium_unavailable"]],
    (report) => assert.equal(report.chromium.installed, false),
  );
  // Reproduce an installed executable that fails to launch without modifying
  // the user's pinned browser installation.
  const resolved = spawnSync(
    node,
    [
      "--input-type=module",
      "-e",
      "import { chromium } from '@playwright/test'; console.log(chromium.executablePath())",
    ],
    {
      cwd: repository,
      env: { ...process.env, PLAYWRIGHT_BROWSERS_PATH: browsers },
      encoding: "utf8",
    },
  );
  assert.equal(resolved.status, 0);
  const chromiumPath = resolved.stdout.trim();
  assert.ok(chromiumPath.startsWith(`${browsers}${path.sep}`));
  await fs.mkdir(path.dirname(chromiumPath), { recursive: true });
  await fs.writeFile(
    chromiumPath,
    `#!/bin/sh\necho '${privatePath}' >&2\nexit 1\n`,
    { mode: 0o700 },
  );
  await check(
    "chromium-launch-failed",
    { PLAYWRIGHT_BROWSERS_PATH: browsers },
    [["chromium", "chromium_unavailable"]],
    (report) => {
      assert.equal(report.chromium.installed, true);
      assert.equal(report.chromium.launchable, false);
    },
  );
  const unsafeCache = await file("unsafe-cache", "unchanged");
  await check(
    "diagnostic-unavailable",
    {
      MENDIMARU_CACHE_DIR: unsafeCache,
      MENDIMARU_BROWSER_RUNNER_PATH: path.join(temporary, "absent.mjs"),
    },
    [["runner", "runner_missing"]],
    (report) => assert.equal(report.diagnostic.stored, false),
  );
  assert.equal(await fs.readFile(unsafeCache, "utf8"), "unchanged");
  const symlinkCache = path.join(temporary, "symlink-cache");
  const outside = path.join(temporary, "outside");
  await fs.mkdir(path.join(symlinkCache, "browser-tests"), { recursive: true });
  await fs.mkdir(outside);
  await fs.symlink(outside, path.join(symlinkCache, "browser-tests/doctor"));
  await check(
    "diagnostic-symlink",
    {
      MENDIMARU_CACHE_DIR: symlinkCache,
      MENDIMARU_BROWSER_RUNNER_PATH: moduleRunner,
    },
    [["js_dependencies", "js_dependencies_missing"]],
    (report) => assert.equal(report.diagnostic.stored, false),
  );
  assert.deepEqual(await fs.readdir(outside), []);
  // Reusing the same cache replaces one private report atomically.
  await check(
    "module-missing",
    { MENDIMARU_BROWSER_RUNNER_PATH: moduleRunner },
    [["js_dependencies", "js_dependencies_missing"]],
  );
  process.stdout.write(`browser doctor: ${count} CLI scenarios passed\n`);
} finally {
  await fs.rm(temporary, { recursive: true, force: true });
}
