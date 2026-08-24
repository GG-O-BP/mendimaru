# AUR publishing

Mendimaru releases are published as the source-built `mendimaru` AUR package.
Pushing a `v`-prefixed semantic-version tag creates the corresponding GitHub
release and publishes the package automatically.

## Package model

- `aur/PKGBUILD` is the maintained packaging template.
- The package is built from the immutable GitHub tag archive and installs the
  application binary, desktop entry, icons, license, and translated readmes.
- The single `mendimaru` package has a required `winboat` dependency. Paru
  installs the exact-name AUR package when the dependency is missing. Existing
  `winboat-bin`, `winboat-electron`, and `winboat-git` packages provide
  `winboat`, so any installed variant satisfies the dependency without being
  replaced.
- Chromium and Google Chrome remain optional alternatives for Marketplace
  discovery.

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
git tag -a v0.2.2 -m "Mendimaru 0.2.2"
git push origin v0.2.2
```

The workflow creates the GitHub release, calculates the source archive's
SHA-256 checksum, regenerates `.SRCINFO`, runs `makepkg --verifysource`, commits
only `PKGBUILD` and `.SRCINFO`, and pushes the AUR `master` branch.

## Local preparation and recovery

The updater defaults to a non-publishing dry run:

```sh
bash scripts/update-mendimaru-aur.sh \
  --tag v0.2.2 \
  --verify-source
```

For a first local publication, add `--initialize-empty --push`. For later
updates, use `--push` without `--initialize-empty`. A manually dispatched
workflow can reprocess an existing release tag and can also run in offline
mode to upload the rendered AUR files as a workflow artifact.

AUR automation does not remove the maintainer's responsibility to review
dependency, license, or build-system changes before every release.
