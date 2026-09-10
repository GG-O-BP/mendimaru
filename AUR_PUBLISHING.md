# AUR publishing

Mendimaru releases are published as the source-built `mendimaru` AUR package.
Pushing a `v`-prefixed semantic-version tag creates the corresponding GitHub
release and publishes the package automatically.

## Package model

- `aur/PKGBUILD` is the maintained packaging template.
- The package is built from the immutable GitHub tag archive and installs the
  application binary, desktop entry, icons, license, translated readmes, and
  browser runtime resources under `/usr/lib/mendimaru/browser/`. The resources
  include `browser-runner.mjs`, `browser-artifact-safety.mjs`, and the complete
  locked `@playwright/test`, `playwright`, `playwright-core`, and `fflate`
  packages, including their licenses. Cargo alone does not copy Tauri resources.
- Node.js `>=22.22.2` is a required runtime dependency. npm is needed only to
  build the package. Browser commands use the installed resources and host Node
  without a source checkout or environment overrides. The package also requires
  `nss` for Chromium's NSS/NSPR shared libraries; the remaining Chromium system
  libraries are provided by the existing desktop dependencies.
- The single `mendimaru` package has a required `winboat` dependency. Paru
  installs the exact-name AUR package when the dependency is missing. Existing
  `winboat-bin`, `winboat-electron`, and `winboat-git` packages provide
  `winboat`, so any installed variant satisfies the dependency without being
  replaced.
- Chromium and Google Chrome remain optional alternatives for Marketplace
  discovery. Browser tests use a separate, pinned Playwright Chromium build;
  only `mendimaru browser install chromium --json` downloads that build.

## Installed-package gate

PR CI and release publication run `scripts/aur/package-gate.sh`. With Docker,
Git, and Node available, run the same gate locally from a committed checkout:

```sh
bash scripts/aur/package-gate.sh
```

The gate builds the current commit with the AUR template in an Arch builder.
It changes only the template's version, archive location, and archive checksum
to select that commit. It extracts the resulting `.pkg.tar.zst` and compares
every browser resource's SHA-256 hash with the locked build inputs and Tauri's
resource map. It also checks the Node runtime dependency and records the full
package inventory.

A second, fresh Arch container installs that package and its runtime
dependencies. It receives only standalone test fixtures, with no source
checkout, npm, Cargo, development dependencies, or runner/Node overrides.
The smoke runs as a normal user and checks missing Node and missing Chromium,
explicit Chromium installation, a ready doctor, and a passing fixture suite
with retrievable artifacts. Networking is disabled for doctor and test;
browser installation is a separate explicit step. The browser cache must stay
unchanged during doctor and test.

Only the unrelated AUR `winboat` dependency is assumed installed in these
disposable containers; the gate does not exercise Studio Pro or a Windows VM.
All other declared dependencies are installed and checked by pacman. Reports,
the tested PKGBUILD, source commit, and package are saved under `artifacts/aur/`
and uploaded by CI. A failed gate blocks the release publication job.

## Repository setup

The release workflow requires these GitHub repository settings:

1. Add a dedicated AUR SSH private key as the Actions secret
   `AUR_SSH_PRIVATE_KEY`. Its public key must be registered on the maintainer's
   AUR account.
2. Set `AUR_INITIALIZE_EMPTY=true` for the first publication only. Set it to
   `false` immediately after the first AUR commit has been published.
3. Optionally set `AUR_GIT_NAME` and `AUR_GIT_EMAIL`. The workflow uses a
   release-automation identity when they are absent.

The workflow verifies the pinned AUR SSH host-key fingerprint before using the
private key. Pull requests do not receive the secret and cannot publish.

## Release

Keep these versions identical before creating a tag:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Then push the release tag:

```sh
git tag -a v0.4.0 -m "Mendimaru 0.4.0"
git push origin v0.4.0
```

The workflow creates the GitHub release, calculates the source archive's
SHA-256 checksum, regenerates `.SRCINFO`, runs `makepkg --verifysource`, commits
only `PKGBUILD` and `.SRCINFO`, and pushes the AUR `master` branch.

## Local preparation and recovery

The updater defaults to a non-publishing dry run:

```sh
bash scripts/update-mendimaru-aur.sh \
  --tag v0.4.0 \
  --verify-source
```

For a first local publication, add `--initialize-empty --push`. For later
updates, use `--push` without `--initialize-empty`. A manually dispatched
workflow can reprocess an existing release tag and can also run in offline
mode to upload the rendered AUR files as a workflow artifact.

AUR automation does not remove the maintainer's responsibility to review
dependency, license, or build-system changes before every release.
