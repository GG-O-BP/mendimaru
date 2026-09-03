<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="public/mendimaru.png" alt="Mendimaru logo" width="180">
</p>

<h1 align="center">Mendimaru</h1>

Mendimaru is a Tauri GUI app for discovering, installing, launching, and removing Mendix Studio Pro versions. It runs natively on Windows and uses WinBoat when running on Linux.

## Interface

- **Studio Pro**: Discover, launch, install, and safely remove Studio Pro versions in the active Windows environment
- **Projects**: Find and launch `.mpr` projects in the configured workspace
- **Operations**: Review persistent install, removal, and launch progress, failures, and retryability
- **Install queue**: Resume verified installer partials and queue multiple Studio Pro versions with reorder, cancel, retry, and restart recovery (see the [install queue guide](docs/install-queue.md))
- **Settings**: Configure a native Windows workspace and optional portable Studio paths, or the WinBoat environment on Linux
- **Portable Runtime**: Build with the project's exact MxBuild and run isolated, readiness-gated web apps on Windows or Linux
- **WinBoat Run Locally**: Mirror a Windows guest Runtime to the same Linux `localhost` port with readiness and Compose rollback

Mendimaru does not provide a dashboard, VM resource information, advanced download URLs, or manual build-number entry.

### Safe project launch

Projects whose exact Studio Pro version is installed open directly. If that version is missing, unknown, or differs from an explicitly selected version, a launch assistant resolves the exact Marketplace release, installs it when needed, verifies that the same version was detected, and only then opens the original `.mpr`. Mendimaru never silently substitutes another installed version. A mismatched or unknown-version launch requires an explicit selection and backup acknowledgement.

The selected version and unfinished launch intent survive cancellation, installation failure, and app restart so the flow can be resumed. This preference store lives in the host-only application configuration directory and identifies projects by a SHA-256 digest of their canonical path; it does not persist project paths.

### Environment diagnostics

Settings checks the WinBoat executable, Compose structure, container runtime daemon, FreeRDP, shared workspace and mount, container state, Guest API, guest clock skew, loopback RDP port, and Marketplace browser independently. A failed check offers only an explicit safe next action such as redetection, starting Windows, opening WinBoat, or focusing the relevant setting. Diagnostic reports can be copied or exported as JSON; they contain allowlisted status fields and omit configured paths, credentials, tokens, and command payloads. See the [WinBoat clock-sync guide](docs/winboat-clock-sync.md) for guest time synchronization.

### Persistent operation history

Install, removal, and launch operations are recorded atomically in the host-only application configuration directory, outside the untrusted shared workspace. The operation center restores this history after a reload or app restart, shows the failed stage, safe reason and Windows exit code when available, and distinguishes retryable work from protected project launches that require the project to be selected again. A running record from a previous app process is marked interrupted instead of trusting an old result whose per-attempt HMAC key no longer exists. Existing Windows report filenames are imported once as untrusted interrupted references; their payloads are never used to infer success.

Clearing completed history removes only terminal host records. It does not delete a running operation, downloaded installer, command script, or Windows report. The history schema stores no project paths, command payloads, URLs, credentials, or HMAC keys.

## Windows installation

Download either the MSI or NSIS setup executable from the GitHub release assets. The Windows build does not require WinBoat, Docker, a Guest API, RDP, FreeRDP, or path conversion.

On native Windows, Mendimaru:

- detects Studio Pro from 32-bit and 64-bit uninstall registry views, standard Mendix folders, Version Selector evidence, and configured custom or portable paths;
- opens `StudioPro.exe` directly and passes a selected `.mpr` path as a process argument;
- keeps project and installer paths as native Windows paths;
- validates the downloaded installer's SHA-256 stability and trusted Authenticode signature from Mendix or Siemens before requesting UAC elevation;
- waits for the elevated installer or official registered uninstaller and accepts the documented success/reboot exit codes;
- refuses removal while that exact Studio Pro executable is running and disables removal when official uninstall metadata is unavailable.

Windows settings are migrated from older configuration files automatically. The optional `windowsStudioPaths` list is empty by default, so existing Linux settings remain valid.

## Arch Linux installation

Install the `mendimaru` package from the AUR:

```bash
paru -S mendimaru
```

WinBoat is a required dependency. If no package satisfying `winboat` is
installed, `paru` installs the AUR `winboat` package automatically. An existing
`winboat`, `winboat-bin`, `winboat-electron`, or `winboat-git` installation
satisfies the dependency and is left in place. To select an alternative on a
new system, install it in the same transaction, for example
`paru -S winboat-bin mendimaru`.

Chromium or Google Chrome is also needed to discover installable Studio Pro versions from the Mendix Marketplace; both are declared as optional browser alternatives.

## Initial WinBoat setup

If WinBoat is installed but its Windows VM has not yet been configured, Mendimaru's **Start WinBoat Setup** button opens the official WinBoat setup wizard. WinBoat handles the Windows account, VM resources, Windows image, and Guest Server installation.

Mendimaru monitors the setup until the wizard finishes, then automatically completes the following tasks:

- Locates the WinBoat executable, including the AUR `winboat-bin` path at `/opt/winboat/winboat`
- Locates `~/.winboat/docker-compose.yml` or `podman-compose.yml`
- Discovers the actual dynamically assigned host ports for the Guest API and RDP from the running container
- Applies the configured Linux workspace to the `/shared` mount in the Compose file
- Backs up the original Compose file as `*.mendimaru.bak` and recreates the container once while preserving the virtual disk

If you cancel or close the initial setup, select **Continue Setup** to reopen the official wizard. Mendimaru does not copy the Windows username or password into its own settings.

## Localization

Mendimaru supports English (`en-US`), Korean (`ko-KR`), and Japanese (`ja-JP`). It initially follows the system language. A language selected from the header menu is saved in the app settings and reused on the next launch. Unsupported system languages fall back to English.

The Rust backend handles translations and locale-sensitive formatting.

- UI text and backend error messages are managed together in the Fluent resources at `src-tauri/i18n/<locale>/mendimaru.ftl`.
- `i18n-embed` embeds translation resources in the executable, selects the system language, and handles the English fallback.
- Dates, numbers, and download sizes are formatted with ICU4X before they are sent to the frontend.
- The frontend displays the translation bundle supplied by the backend and does not infer state from translated text. Values that affect behavior, such as download cancellation, are delivered as separate codes and state.
- Tests verify that every language has the same translation keys and variables and that the backend bundle contains all static translation keys used by React.

To add a language, register its BCP 47 language tag and display name in the supported-locale list in `src-tauri/src/i18n.rs`, then add a Fluent file with the same keys and variables as the English file at `src-tauri/i18n/<locale>/mendimaru.ftl`. Add new UI text to all three Fluent files and to `src/shared/contracts/uiMessages.json`, which supplies both the TypeScript translation-key type and the Rust UI bundle. `cargo test` detects missing translations and variable mismatches.

## Discovering and installing Studio Pro versions

Mendimaru uses Chromium to read the data grid on the [Mendix Marketplace Studio Pro page](https://marketplace.mendix.com/link/studiopro), following the same approach as `kirakiraichigo-mendix-manager`.

- It automatically refreshes the first 10 versions and fetches the next page when you select **Load Older Versions**.
- It stores the list in `studio-version-catalog.json` in the app cache directory so it can be shown immediately on the next launch.
- It retrieves release dates together with the Latest, LTS, MTS, and Beta labels.
- Studio Pro 11 and later use the official `Mendix-<version>-Setup.exe` artifact.
- For Studio Pro 10 and earlier, Mendimaru extracts `Build <number>` from the version details page and uses `Mendix-<version>.<build>-Setup.exe`.
- You only need to select a version from the list; there is no need to enter a URL or build number.
- A completed installer is kept in the host-private application cache and reused only after its recorded source, expected size, Windows PE structure, and SHA-256 all validate. Legacy installers in the shared workspace are not migrated or trusted. Downloads use create-new CSPRNG temporary names, accept only the exact HTTPS Mendix artifact origin across every redirect, and enforce a 2 GiB limit from both `Content-Length` and streamed bytes.
- Each uninstalled catalog version provides a force-redownload action for recovering from an installer failure without reusing the existing cache.

On Windows, Microsoft Edge and Chrome are detected in their standard per-machine and per-user locations. On Linux, browser detection checks `MENDIMARU_CHROME_PATH`, `google-chrome-stable`, `google-chrome`, `chromium`, and `chromium-browser`.

## Windows paths

Native discovery uses these default locations in addition to registry and Version Selector evidence.

| Purpose | Windows path |
| --- | --- |
| Studio Pro installation root | `C:\Program Files\Mendix` |
| Studio Pro executable | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro uninstall information | `C:\ProgramData\Mendix` |
| Native default workspace | `%USERPROFILE%\Mendix` (the app asks for another directory if it does not exist) |
| Linux WinBoat shared path | `\\host.lan\Data` |

Installers are stored outside the configured workspace in the host-private application cache. In native mode, the installer is signature-checked and launched through the Windows elevation API without a command shell. In Linux mode, a verified cache descriptor is copied into a fresh create-new file under `.mendimaru/installers` only for the duration of the WinBoat installation; Windows verifies the pinned SHA-256 and Authenticode signature, makes its own private staging copy, and the host staging file is then removed. Commands are sent to WinBoat RemoteApp as UTF-16LE-encoded PowerShell, avoiding quoting issues. Installation is complete only after the installer exits successfully and `StudioPro.exe` for that version is detected.

The Marketplace catalog cache is reused for up to six hours so normal startup does not launch a background browser. Use the catalog refresh control to force an immediate update.

Likewise, removal is complete only after the official Windows uninstaller exits and `StudioPro.exe` for that version disappears. The installed-version list is then refreshed automatically.

At startup, the GUI restores the last verified installed-version list from a private host cache so known Studio Pro releases appear immediately. The current Windows list is verified in the background; install, remove, launch, and project-open actions remain locked until that verification succeeds. If verification fails, the last known list remains visible with an explicit retry instead of being treated as an empty installation.

When WinBoat Guest advertises `apps-query-v1`, Mendimaru authenticates with WinBoat's private shared token and requests only icon-free `Name`, `Path`, and `Source` fields below the configured Mendix root. Older Guests safely fall back to the bounded full `/apps` response; an advertised lightweight request is never silently downgraded after an authentication, timeout, or validation failure. See [WinBoat Studio discovery](docs/winboat-studio-discovery.md).

In Linux WinBoat mode, the Studio Pro launch button remains disabled until the Windows process has created a real window and FreeRDP is ready to display it. WinBoat can retain only one connected Studio Pro RemoteApp at a time, so while one is connected Mendimaru locks new Studio launches, project opening, installation, and removal until that session is closed from the Studio Pro view. The same actions stay locked while a launch is being prepared. Windows hash-pins the shared operation script, copies it to a unique private path, and executes only that copy. Installation and removal inherit the token of the already elevated WinBoat session, so no separate UAC window is shown.

## Linux shared workspace

The Linux shared directory is connected to the `<host path>:/shared` mount in the WinBoat Compose file. It is treated as an untrusted transport, not as installer storage. The project list scans only this directory and excludes generated and cache directories such as `.git`, `node_modules`, `deployment`, `.mendix-cache`, and `.mendimaru`.

Project discovery runs off the UI backend path and is bounded to depth 8, 100,000 visited entries, 10,000 projects, 256 KiB per `project-settings.user.json`, 16 MiB of settings in one scan, and 5 seconds. Larger trees and unreadable or non-regular settings files produce a partial report with skipped/error counts instead of looking like silent success; the UI renders the first 100 matching projects and offers more. Workspace watchers are debounced and coalesced, unsupported watchers fall back to a 30-second refresh (with a five-minute watcher safety refresh), and an old workspace response cannot overwrite a newer workspace result. Favorites and last-open timestamps are persisted only as hashed project identities; favorites for projects that are no longer discovered are removed automatically.

The Projects page can also open one explicitly selected `.mpr` outside this workspace. Mendimaru canonicalizes the selection, rejects symlinked files or parent paths, and redirects only the `.mpr` file's direct parent directory as a writable, session-scoped FreeRDP drive on the same retained RemoteApp connection. The project is not copied, synchronized, added to Compose, or added permanently to the project list. Windows waits up to 30 seconds for the redirected `.mpr` to become a real file before starting the exact Studio Pro executable, so saves apply to the original Linux project.

The generated share name is bounded ASCII derived from a path digest and does not expose the host directory name. The GUI selection receives only a bounded current-process token derived from that digest; raw Linux paths are kept out of serialized selection and launch DTOs, operation history, session DTOs, Windows reports, diagnostics, and logs. Closing Studio Pro closes the retained FreeRDP process and the temporary drive. If the app or RemoteApp connection ends while Studio remains open, the session is marked non-reconnectable because its temporary drive no longer exists; stop it or select the same `.mpr` again to begin a new protected launch. A comma, backslash, newline, non-UTF-8 path, filesystem root, home directory, or read-only project directory is rejected before launch because it cannot be represented or scoped safely.

Native FreeRDP must be able to read the selected directory. For a sandboxed/Flatpak FreeRDP wrapper, grant that directory explicitly with the packaging system's filesystem permission control and select the project again; Mendimaru does not broaden the share to the home directory. Use FreeRDP 3 from a currently patched distribution, since the project drive is writable by the Windows session.

When the shared directory changes, Mendimaru first previews the identified WinBoat service, the old and new `/shared` sources, and the restart scope. It rejects non-WinBoat or ambiguous Compose files and aborts if the file changes between preview and save. Mendimaru backs up the existing Compose file as `*.mendimaru.bak` and recreates only the identified service when immediate apply is enabled. `/storage`, other volumes, environment, networks, and unrelated services are preserved semantically; because the Compose document is serialized after the targeted edit, YAML formatting and comments may be normalized.

## Backend capability contract

Agents and CI can inspect the platform-neutral backend contract without starting the GUI:

```bash
mendimaru capabilities --json
```

The response separates the host, Studio, and optional Runtime platforms and reports every Studio, Runtime, UI automation, and browser action as supported or unsupported. An explicit `--backend` must match the current host; Mendimaru never silently falls back to another backend. See [Platform backend and capability contract](docs/backend-contract.md) and the machine-readable [JSON Schemas](schemas/).

### Headless CLI

The installed `mendimaru` executable can inspect or ensure the environment, list/install/remove/start exact Studio Pro versions, query/stop Studio sessions, resolve projects by opaque ID, and query/retry persistent operations without initializing Tauri or opening a dialog. Results are JSON on stdout, errors are JSON on stderr, and `--ndjson` adds structured progress events. `--timeout-seconds` and `Ctrl+C` cancel at the shared operation boundary; interrupted work can be re-queried by operation ID. See the [headless CLI contract](docs/headless-cli.md) for the full command surface, exit codes, schemas, and safety rules.

Linux headless sessions can also prepare Studio Pro Run Locally forwarding with `runtime start --mode studio-run-locally`, then use the common `wait`, `url`, `status`, `logs`, and `stop` commands. See the [WinBoat Run Locally guide](docs/winboat-run-locally.md) for the loopback-only exposure and recovery boundary.

On Linux, `browser test` runs the same declarative Playwright/Chromium suite against an explicit URL, a Portable Runtime session, or a WinBoat Run Locally session. Browser downloads are explicit, and failed runs retain masked HTML, DOM/accessibility, screenshot, trace, console, and network evidence under bounded retention. See the [browser testing guide](docs/browser-testing.md).

## Development

Development requires Node.js 22.22.2 or later, Rust, and the Tauri system dependencies for the host platform. Linux integration additionally requires WinBoat, Docker or Podman, FreeRDP 3, and Chrome or Chromium. Native Windows catalog discovery uses Edge or Chrome.

```bash
npm install
npm run tauri dev
```

Set `MENDIMARU_STUDIO_TRACE=1` while running `npm run tauri dev` to emit bounded Studio overview timing and coalescing diagnostics. The trace contains durations, payload sizes, and item counts, but no configured paths or raw Guest payloads.

To run the host-portable validation suite and build an application bundle:

```bash
npm run check:portable
npm run test:browser
npm run test:e2e
npm run test:e2e:coverage
npm run check:windows
npm run tauri build
```

On Linux, `npm run test:e2e` launches the debug executable against the Vite development URL through pinned `tauri-driver` and `WebKitWebDriver`. It uses isolated WinBoat/API/project fixtures and verifies the real WebView, Tauri IPC, online application state, project discovery, CSP enforcement, hostile-input rejection, sampled persistent online-route and scoped busy-state motion, the absence of every non-allowlisted idle animation, primary navigation, and startup/IPC/navigation/private-memory/idle-CPU budgets. Install the driver bridge with `cargo install tauri-driver --version 2.0.6 --locked`; the host must also provide `WebKitWebDriver`. `npm run test:app-flow` retains the faster React application-flow suite with mocked OS boundaries, while `npm run test:browser` tests Mendix Runtime pages rather than the Mendimaru desktop shell. CI gates all three layers and records Linux E2E measurements and screenshots. Optimized AppImage, WebView2, MSI, and NSIS measurements use a separate same-host baseline gate documented in the [release performance contract](docs/release-performance.md). The complete motion inventory and change policy are documented in [Motion contract](docs/motion-contract.md).

`npm run test:e2e:coverage` verifies the checked-in E2E coverage model. Linux and Windows now have parity for the core real-desktop functional, security, and performance gates, but they do **not** have full platform parity: hosted Linux CI has no real WinBoat lifecycle, no AUR/package install-launch-uninstall lifecycle comparable to MSI/NSIS, and no live Marketplace refresh. The generated report is `artifacts/e2e/e2e-coverage.json`.

On Windows, `npm run test:e2e:windows` starts the real application through `tauri dev` with a test-only Cargo feature and drives the native WebView2 window through an embedded WebDriver endpoint. It exercises real IPC, registry discovery, project scanning, settings persistence, Edge-backed Marketplace refresh, CSP enforcement, hostile-input rejection, and performance budgets. Configuration and caches are restricted to a safety-marked temporary directory that is removed afterward. The WebDriver feature, permission, and global Tauri bridge are absent from normal development and release builds.

CI audits both npm and Cargo lockfiles and runs the native Windows E2E. It then builds, installs, launches, and uninstalls MSI and NSIS packages on a marked ephemeral Windows VM; the installer lifecycle script refuses to run on an ordinary workstation. If `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD`, and `WINDOWS_TIMESTAMP_URL` are all configured, the workflow signs and timestamps every Windows artifact and verifies Authenticode before upload. If none are configured, it publishes installers only after the same lifecycle checks and clearly marks them as unsigned in the release notes and an attached `WINDOWS-BUILDS-UNSIGNED.txt`; a partial signing configuration fails the release.

Run `npm run test:winboat-smoke` for the non-destructive RemoteApp gate against an online WinBoat VM. It verifies authenticated session queries and stale-session rejection. On Linux, the exhaustive `npm run check` command also requires the destructive lifecycle gate instead of silently excluding it. Supply an absent disposable version and explicit mutation permission:

```bash
MENDIMARU_E2E_ALLOW_MUTATION=1 \
MENDIMARU_E2E_VERSION=11.13.0 \
npm run check
```

Use `npm run test:winboat-e2e` with the same environment variables to run only the lifecycle. The exact disposable version must have an official installer in the host-private application cache, normally created by an earlier Mendimaru download. The lifecycle refuses a preinstalled target and verifies absent → installed → real Studio window → running-removal rejection → graceful close → uninstalled, including progress ordering, exact process identity, unchanged pre-existing installations and private installer cache, removal of the unique shared staging file, stale/repeated action rejection, and no leaked processes or unexpected RemoteApp/PowerShell windows. Both live gates use isolated Xvfb and require `xvfb-run`, `xfwm4`, and `wmctrl`; on Arch Linux, `xvfb-run` is provided by `xorg-server-xvfb`. Other host platforms report the WinBoat lifecycle as not applicable. Because hosted CI has no live WinBoat VM, it runs the portable component gates rather than claiming this local live result.

The full Rust suite covers registry parsing, path containment, installer integrity, Windows argument quoting, UAC/exit-code failures, and a fixture-backed install-to-uninstall lifecycle.

Serialized enum values shared by Rust and TypeScript are registered in `src/shared/contracts/enumValues.json`. TypeScript derives its union types from this registry, and a Rust test rejects contract drift.

Live Marketplace integration tests are excluded from the default test run. Run them with:

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## Security

Native Windows commands never interpolate paths into a command shell. Installers, installed Studio executables, and registered Mendix uninstallers must have a valid trusted Authenticode signature whose publisher is Mendix or Siemens. A verified executable remains open with write/delete sharing denied until Windows starts it, closing the replacement window between signature verification and execution. Windows Installer removal is limited to a product-code `/x` operation and known non-interactive flags, while registered uninstallers must belong to the selected installation and use an allowlisted flag set. UAC cancellation and non-success process exit codes leave the operation failed rather than reporting a false install or removal.

On Linux, Mendimaru does not store the Windows username or password in its app settings. When launching a RemoteApp, it reads credentials from the running WinBoat container and passes them to FreeRDP 3 through standard input, keeping the password out of process arguments and app logs. FreeRDP uses an app-scoped TOFU certificate pin, and privileged operations require loopback-only Guest API and RDP bindings. Shared operation results and retained Studio session-control requests are authenticated with per-attempt HMAC keys and replay-protected sequence numbers.

See [Security policy and WinBoat trust boundary](SECURITY.md) for the threat model, executable trust chain, container privileges, residual risks, and reporting guidance.

## License

Mendimaru is available under the [MIT License](LICENSE).
