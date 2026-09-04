import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const root = path.resolve(import.meta.dirname, "..");
const artifactDirectory = path.join(root, "artifacts", "e2e");
const [
  packageJson,
  ci,
  linuxDesktop,
  windowsDesktop,
  windowsBundle,
  winboat,
  marketplaceSession,
] = await Promise.all([
  read("package.json"),
  read(".github/workflows/ci.yml"),
  read("scripts/test-tauri-e2e.mjs"),
  read("scripts/e2e/windows-native.mjs"),
  read("scripts/e2e/windows-bundle-smoke.ps1"),
  read("scripts/test-winboat-e2e.mjs"),
  read("src-tauri/src/marketplace/browser/session.rs"),
]);
const scripts = JSON.parse(packageJson).scripts;

const marketplaceSandboxGate =
  hasAll(ci, [
    "Run sandboxed Marketplace Chromium security gate",
    "live_linux_marketplace_browser_security_gate",
    "command -v google-chrome-stable",
    'MENDIMARU_CHROME_PATH="$chrome_path"',
  ]) &&
  hasAll(marketplaceSession, [
    "GetProcessInfoParams",
    "NoNewPrivs:",
    "Seccomp:",
    "renderer_process_is_sandboxed",
    "profile_process_ids",
  ]);

const linux = {
  realDesktopWebView: hasAll(linuxDesktop, [
    'browserName: "wry"',
    "window.__TAURI_INTERNALS__.invoke",
  ]),
  functionalDesktop: hasAll(linuxDesktop, [
    'invoke("get_environment_status")',
    'invoke("get_projects")',
    '"Projects"',
    '"Operation center"',
    '"Settings"',
    '"Studio Pro"',
    "assertStudioCatalogLayout",
    "assertProjectsLayout",
    "waitForSuccessfulLaunchOperation",
    "select-external-project",
  ]),
  securityDesktop:
    hasAll(linuxDesktop, [
      'VITE_MENDIMARU_E2E: "1"',
      "window.__MENDIMARU_CSP_PROBE__",
      '"11.12.2; calc.exe"',
      "legacy shared-cache partial symlink",
      '"../11.12.2"',
      "expectedSharedDirectory",
    ]) && marketplaceSandboxGate,
  marketplaceSandboxGate,
  performanceDesktop: hasAll(linuxDesktop, [
    "MENDIMARU_E2E_MAX_STARTUP_MS",
    "MENDIMARU_E2E_MAX_ENVIRONMENT_MS",
    "MENDIMARU_E2E_MAX_PROJECTS_MS",
    "MENDIMARU_E2E_MAX_NAVIGATION_MS",
    "MENDIMARU_E2E_MAX_PRIVATE_MEMORY_BYTES",
    "MENDIMARU_E2E_MAX_IDLE_CPU_PERCENT",
    "linuxProcessSnapshot",
  ]),
  runtimeBrowser: ci.includes("npm run test:browser"),
  hostedDesktopCi: ci.includes("xvfb-run --auto-servernum npm run test:e2e"),
  localLiveStudioLifecycle:
    scripts["test:winboat-e2e"]?.includes("--lifecycle") &&
    winboat.includes("live_e2e_linux_winboat_backend_lifecycle"),
  hostedLiveStudioLifecycle: linuxCiIncludes("npm run test:winboat-e2e"),
  hostedPackageLifecycle:
    linuxCiIncludes("makepkg") &&
    linuxCiIncludes("pacman") &&
    linuxCiIncludes("mendimaru"),
  hostedLiveMarketplace: linuxCiIncludes("live_scrapes_the_first_catalog_page"),
};

const windows = {
  realDesktopWebView: hasAll(windowsDesktop, [
    'browserName: "tauri"',
    "waitForWebDriverSession",
  ]),
  functionalDesktop: hasAll(windowsDesktop, [
    'client.invoke("get_environment_status")',
    'client.invoke("get_projects")',
    'client.invoke("get_installed_versions")',
    "native settings UI persists a workspace",
  ]),
  securityDesktop: hasAll(windowsDesktop, [
    "window.__MENDIMARU_CSP_PROBE__",
    '"11.12.2; calc.exe"',
    '"../11.12.2"',
    "isolated workspace",
  ]),
  performanceDesktop: hasAll(windowsDesktop, [
    "MENDIMARU_E2E_MAX_STARTUP_MS",
    "MENDIMARU_E2E_MAX_ENVIRONMENT_MS",
    "MENDIMARU_E2E_MAX_PROJECTS_MS",
    "MENDIMARU_E2E_MAX_NAVIGATION_MS",
    "MENDIMARU_E2E_MAX_PRIVATE_MEMORY_BYTES",
    "MENDIMARU_E2E_MAX_IDLE_CPU_PERCENT",
    "windowsProcessSnapshot",
  ]),
  hostedDesktopCi: ci.includes("npm run test:e2e:windows"),
  hostedPackageLifecycle: hasAll(ci, [
    "Build, install, launch, and uninstall Windows bundles",
    "windows-bundle-smoke.ps1",
  ]),
  packageSecurity: hasAll(windowsBundle, [
    "Get-AuthenticodeSignature",
    "TimeStamperCertificate",
    "Test-PathWithin",
  ]),
  hostedLiveMarketplace: windowsDesktop.includes(
    "real Edge-backed Marketplace refresh",
  ),
};

const sharedDesktopCategories = [
  "realDesktopWebView",
  "functionalDesktop",
  "securityDesktop",
  "performanceDesktop",
  "hostedDesktopCi",
];
const coreDesktopParity = sharedDesktopCategories.every(
  (category) => linux[category] && windows[category],
);

// The static claims above prove only that the assertions exist in the source.
// When a real Linux E2E run is present (CI always runs it before this gate),
// corroborate those claims against the assertions that actually executed so a
// commented-out or dead-code assertion cannot silently pass this verifier.
const runReport = await readOptional("artifacts/e2e/linux-tauri.json");
let executedLinuxDesktop = null;
if (runReport) {
  const run = JSON.parse(runReport);
  const assertions = Array.isArray(run.assertions) ? run.assertions : [];
  const executed = (fragment) =>
    assertions.some((assertion) => assertion.includes(fragment));
  executedLinuxDesktop = {
    status: run.status ?? "unknown",
    assertionCount: assertions.length,
    security:
      executed("restrictive development CSP") &&
      executed("blocks a data-script CSP probe") &&
      executed("rejects hostile or missing input") &&
      executed("legacy shared-cache partial symlink"),
    performance:
      executed("startupMs") &&
      executed("environmentMs") &&
      executed("navigationMs") &&
      executed("private memory stays below") &&
      executed("idle CPU stays below"),
    functional:
      executed("project scanner finds the isolated Orders fixture") &&
      executed("reports a ready WinBoat environment") &&
      executed("catalog timestamp has separation") &&
      executed("non-overlapping geometry") &&
      executed("clicking Open completes a protected project launch") &&
      executed("external project selection remains responsive"),
  };
  assert.equal(
    run.status,
    "passed",
    "The recorded Linux E2E run must be green before trusting its coverage.",
  );
  assert.ok(
    executedLinuxDesktop.security &&
      executedLinuxDesktop.performance &&
      executedLinuxDesktop.functional,
    `Recorded Linux E2E assertions do not corroborate the static coverage claims: ${JSON.stringify(
      executedLinuxDesktop,
      null,
      2,
    )}`,
  );
}

const gaps = [
  !linux.hostedLiveStudioLifecycle &&
    "Hosted Linux CI has no real WinBoat install → launch → window → stop → uninstall lifecycle.",
  !linux.hostedPackageLifecycle &&
    "Linux has no CI package install → native launch → uninstall lifecycle comparable to MSI/NSIS.",
].filter(Boolean);
const fullPlatformParity =
  coreDesktopParity &&
  linux.hostedLiveStudioLifecycle &&
  linux.hostedPackageLifecycle &&
  windows.hostedPackageLifecycle &&
  windows.hostedLiveMarketplace;

assert.ok(
  coreDesktopParity,
  `Linux/Windows core desktop E2E parity is incomplete: ${JSON.stringify(
    { linux, windows },
    null,
    2,
  )}`,
);
assert.equal(
  fullPlatformParity,
  false,
  "Update the parity model when Linux gains all hosted platform lifecycle gates.",
);

const report = {
  schemaVersion: "1.0.0",
  generatedAt: new Date().toISOString(),
  coreDesktopParity,
  fullPlatformParity,
  sameOverallLevel: fullPlatformParity,
  linux,
  windows,
  executedLinuxDesktop,
  gaps,
};
await mkdir(artifactDirectory, { recursive: true });
await writeFile(
  path.join(artifactDirectory, "e2e-coverage.json"),
  `${JSON.stringify(report, null, 2)}\n`,
);

process.stdout.write(
  [
    `E2E coverage: core desktop parity=${coreDesktopParity}, full platform parity=${fullPlatformParity}.`,
    executedLinuxDesktop
      ? `Corroborated by the recorded Linux E2E run (${executedLinuxDesktop.assertionCount} assertions, status=${executedLinuxDesktop.status}).`
      : "No recorded Linux E2E run found; verified statically only.",
    ...gaps.map((gap) => `- ${gap}`),
    "",
  ].join("\n"),
);

async function read(relativePath) {
  return readFile(path.join(root, relativePath), "utf8");
}

async function readOptional(relativePath) {
  try {
    return await readFile(path.join(root, relativePath), "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return undefined;
    throw error;
  }
}

function hasAll(content, fragments) {
  return fragments.every((fragment) => content.includes(fragment));
}

function linuxCiIncludes(fragment) {
  const linuxSteps = ci
    .split(/\n(?= {6}- name:)/)
    .filter((step) => step.includes("runner.os == 'Linux'"))
    .join("\n");
  return linuxSteps.includes(fragment);
}
