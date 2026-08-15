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
- **Settings**: Configure a native Windows workspace and optional portable Studio paths, or the WinBoat environment on Linux
- **Portable Runtime**: Build with the project's exact MxBuild and run isolated, readiness-gated web apps on Windows or Linux
- **WinBoat Run Locally**: Forward a Windows guest Runtime to a dynamic Linux loopback port with readiness and Compose rollback

Mendimaru does not provide a dashboard, VM resource information, advanced download URLs, or manual build-number entry.

### Safe project launch

Projects whose exact Studio Pro version is installed open directly. If that version is missing, unknown, or differs from an explicitly selected version, a launch assistant resolves the exact Marketplace release, installs it when needed, verifies that the same version was detected, and only then opens the original `.mpr`. Mendimaru never silently substitutes another installed version. A mismatched or unknown-version launch requires an explicit selection and backup acknowledgement.

The selected version and unfinished launch intent survive cancellation, installation failure, and app restart so the flow can be resumed. This preference store lives in the host-only application configuration directory and identifies projects by a SHA-256 digest of their canonical path; it does not persist project paths.

### Environment diagnostics

Settings checks the WinBoat executable, Compose structure, container runtime daemon, FreeRDP, shared workspace and mount, container state, Guest API, loopback RDP port, and Marketplace browser independently. A failed check offers only an explicit safe next action such as redetection, starting Windows, opening WinBoat, or focusing the relevant setting. Diagnostic reports can be copied or exported as JSON; they contain allowlisted status fields and omit configured paths, credentials, tokens, and command payloads.

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
- A completed installer is reused only after its recorded source, expected size, Windows PE structure, and SHA-256 all validate. Invalid legacy or modified caches are removed and downloaded again.
- Each uninstalled catalog version provides a force-redownload action for recovering from an installer failure without reusing the existing cache.

On Windows, Microsoft Edge and Chrome are detected in their standard per-machine and per-user locations. On Linux, browser detection checks `MENDIMARU_CHROME_PATH`, `google-chrome-stable`, `google-chrome`, `chromium`, and `chromium-browser`.

## Windows paths

Native discovery uses these default locations in addition to registry and Version Selector evidence.

| Purpose | Windows path |
| --- | --- |
| Studio Pro installation root | `C:\Program Files\Mendix` |
| Studio Pro executable | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro uninstall information | `C:\ProgramData\Mendix` |
| Native default workspace | `%USERPROFILE%\Mendix` when present, otherwise `%USERPROFILE%` |
| Linux WinBoat shared path | `\\host.lan\Data` |

Installers are stored in `.mendimaru/installers` under the configured workspace. In native mode, the installer is signature-checked and launched through the Windows elevation API without a command shell. In Linux mode, commands are sent to WinBoat RemoteApp as UTF-16LE-encoded PowerShell, avoiding quoting issues. Installation is complete only after the installer exits successfully and `StudioPro.exe` for that version is detected.

Likewise, removal is complete only after the official Windows uninstaller exits and `StudioPro.exe` for that version disappears. The installed-version list is then refreshed automatically.

In Linux WinBoat mode, the Studio Pro launch button remains disabled until the Windows process has created a real window and FreeRDP is ready to display it. While a launch is being prepared, launch buttons for other versions and projects are also locked to prevent duplicate launches. Windows hash-pins the shared operation script, copies it to a unique private path, and executes only that copy. Installation and removal inherit the token of the already elevated WinBoat session, so no separate UAC window is shown.

## Linux shared workspace

The Linux shared directory is connected to the `<host path>:/shared` mount in the WinBoat Compose file. The project list scans only this directory and excludes generated and cache directories such as `.git`, `node_modules`, `deployment`, `.mendix-cache`, and `.mendimaru`.

When the shared directory changes, Mendimaru backs up the existing Compose file as `*.mendimaru.bak`. If you choose to apply the change immediately in Settings, Mendimaru recreates the WinBoat container while preserving the `/storage` virtual disk and installed Windows apps.

## Backend capability contract

Agents and CI can inspect the platform-neutral backend contract without starting the GUI:

```bash
mendimaru capabilities --json
```

The response separates the host, Studio, and optional Runtime platforms and reports every Studio, Runtime, UI automation, and browser action as supported or unsupported. An explicit `--backend` must match the current host; Mendimaru never silently falls back to another backend. See [Platform backend and capability contract](docs/backend-contract.md) and the machine-readable [JSON Schemas](schemas/).

### Headless CLI

The installed `mendimaru` executable can inspect or ensure the environment, list/install/remove/start exact Studio Pro versions, query/stop Studio sessions, resolve projects by opaque ID, and query/retry persistent operations without initializing Tauri or opening a dialog. Results are JSON on stdout, errors are JSON on stderr, and `--ndjson` adds structured progress events. `--timeout-seconds` and `Ctrl+C` cancel at the shared operation boundary; interrupted work can be re-queried by operation ID. See the [headless CLI contract](docs/headless-cli.md) for the full command surface, exit codes, schemas, and safety rules.

Linux headless sessions can also prepare Studio Pro Run Locally forwarding with `runtime start --mode studio-run-locally`, then use the common `wait`, `url`, `status`, `logs`, and `stop` commands. See the [WinBoat Run Locally guide](docs/winboat-run-locally.md) for the loopback-only exposure and recovery boundary.

## Development

Development requires Node.js 22.22.2 or later, Rust, and the Tauri system dependencies for the host platform. Linux integration additionally requires WinBoat, Docker or Podman, FreeRDP 3, and Chrome or Chromium. Native Windows catalog discovery uses Edge or Chrome.

```bash
npm install
npm run tauri dev
```

To validate the project and build an application bundle:

```bash
npm run check
npm run tauri build
```

`npm run test:e2e` runs the native Windows application-flow suite with mocked OS boundaries. The full Rust suite covers registry parsing, path containment, installer integrity, Windows argument quoting, UAC/exit-code failures, and a fixture-backed install-to-uninstall lifecycle. CI runs the frontend and Rust suites on both Windows and Linux and smoke-builds MSI and NSIS installers on Windows.

Serialized enum values shared by Rust and TypeScript are registered in `src/shared/contracts/enumValues.json`. TypeScript derives its union types from this registry, and a Rust test rejects contract drift.

Live Marketplace integration tests are excluded from the default test run. Run them with:

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## Security

Native Windows commands never interpolate paths into a command shell. Installers, installed Studio executables, and registered Mendix uninstallers must have a valid trusted Authenticode signature whose publisher is Mendix or Siemens; files are hashed before and after verification to detect replacement. Windows Installer removal is limited to a product-code `/x` operation and known non-interactive flags, while registered uninstallers must belong to the selected installation and use an allowlisted flag set. UAC cancellation and non-success process exit codes leave the operation failed rather than reporting a false install or removal.

On Linux, Mendimaru does not store the Windows username or password in its app settings. When launching a RemoteApp, it reads credentials from the running WinBoat container and passes them to FreeRDP 3 through standard input, keeping the password out of process arguments and app logs. FreeRDP uses an app-scoped TOFU certificate pin, and privileged operations require loopback-only Guest API and RDP bindings. Shared operation results are authenticated with per-attempt HMAC keys and replay-protected sequence numbers.

See [Security policy and WinBoat trust boundary](SECURITY.md) for the threat model, executable trust chain, container privileges, residual risks, and reporting guidance.

## License

Mendimaru is available under the [MIT License](LICENSE).
