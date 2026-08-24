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
- **Settings**: Configure a native Windows workspace and optional portable Studio paths, or the WinBoat environment on Linux

Mendimaru does not provide a dashboard, VM resource information, advanced download URLs, manual build-number entry, or a force-redownload option.

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

Installers are stored in `.mendimaru/installers` under the configured workspace. In native mode, the installer is signature-checked and launched through the Windows elevation API without a command shell. In Linux mode, commands are sent to WinBoat RemoteApp as UTF-16LE-encoded PowerShell, avoiding quoting issues. Installation is complete only after the installer exits successfully and `StudioPro.exe` for that version is detected.

The Marketplace catalog cache is reused for up to six hours so normal startup does not launch a background browser. Use the catalog refresh control to force an immediate update.

Likewise, removal is complete only after the official Windows uninstaller exits and `StudioPro.exe` for that version disappears. The installed-version list is then refreshed automatically.

In Linux WinBoat mode, the Studio Pro launch button remains disabled until the Windows process has created a real window and FreeRDP is ready to display it. While a launch is being prepared, launch buttons for other versions and projects are also locked to prevent duplicate launches. The launch script is stored in the shared directory, and only a short invocation command is passed to RemoteApp so it stays within FreeRDP RAIL's command-length limit. Windows Script Host runs PowerShell in hidden mode. Installation and removal inherit the token of the already elevated WinBoat session, so neither a PowerShell console nor a separate UAC window is shown.

## Linux shared workspace

The Linux shared directory is connected to the `<host path>:/shared` mount in the WinBoat Compose file. The project list scans only this directory and excludes generated and cache directories such as `.git`, `node_modules`, `deployment`, `.mendix-cache`, and `.mendimaru`.

When the shared directory changes, Mendimaru backs up the existing Compose file as `*.mendimaru.bak`. If you choose to apply the change immediately in Settings, Mendimaru recreates the WinBoat container while preserving the `/storage` virtual disk and installed Windows apps.

## Development

Development requires Node.js 22.22.2 or later, Rust, and the Tauri system dependencies for the host platform. Linux integration additionally requires WinBoat, Docker or Podman, FreeRDP 3, and Chrome or Chromium. Native Windows catalog discovery uses Edge or Chrome.

```bash
npm install
npm run tauri dev
```

To validate the project and build an application bundle:

```bash
npm run check
npm run check:windows
npm run tauri build
```

`npm run test:ui:integration` runs the fast React flow with mocked OS boundaries. On Windows, `npm run test:e2e` starts the real application through `tauri dev` with a test-only Cargo feature, drives the native WebView2 window over an embedded WebDriver endpoint, and exercises real IPC, registry discovery, project scanning, settings persistence, Edge-backed Marketplace refresh, CSP enforcement, invalid-input rejection, and performance budgets. Its config and cache are restricted to a safety-marked temporary directory, which is removed after the run. The WebDriver feature, permissions, and global Tauri bridge are excluded from normal development and release builds.

CI runs the frontend and Rust suites on Windows and Linux, audits both npm and Cargo lockfiles, runs the actual native E2E on Windows, and then builds, installs, launches, and uninstalls both MSI and NSIS packages on a marked ephemeral Windows VM. Bundle installation tests refuse to run on an ordinary workstation.

Windows releases are required to be Authenticode-signed. Configure `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD` as repository secrets and `WINDOWS_TIMESTAMP_URL` as a repository variable. Before creating a release, the workflow repeats the complete Windows checks, Rust audit, and native E2E. It then imports the PFX temporarily, signs the application and both installers, verifies every signature, installs/launches/uninstalls both signed bundles on the marked ephemeral VM, and refuses to upload an unverified artifact.

Serialized enum values shared by Rust and TypeScript are registered in `src/shared/contracts/enumValues.json`. TypeScript derives its union types from this registry, and a Rust test rejects contract drift.

Live Marketplace integration tests are excluded from the default test run. Run them with:

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## Security

Native Windows commands never interpolate paths into a command shell. Installers, installed Studio executables, and registered Mendix uninstallers must have a valid trusted Authenticode signature whose publisher is Mendix or Siemens. The verified file remains open with write/delete sharing denied until Windows starts it, closing the replacement window between signature checking and execution. Windows Installer removal is limited to a product-code `/x` operation and known non-interactive flags, while registered uninstallers must belong to the selected installation and use an allowlisted flag set. UAC cancellation and non-success process exit codes leave the operation failed rather than reporting a false install or removal.

On Linux, Mendimaru does not store the Windows username or password in its app settings. When launching a RemoteApp, it reads credentials from the running WinBoat container and passes them to FreeRDP 3 through standard input, keeping the password out of process arguments and app logs.

## License

Mendimaru is available under the [MIT License](LICENSE).
