# CI gate relevance

Four expensive `ci.yml` gates are relevance-classified so that a change which
cannot affect them does not pay for them:

| Gate          | Job                                                     | Runs when                                                                                 |
| ------------- | ------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `bundle`      | `Build, install, launch, and uninstall Windows bundles` | Application sources, packaged resources, build inputs, or the bundle smoke harness change |
| `windows_e2e` | `Actual Windows Tauri dev E2E`                          | Application sources, packaged resources, build inputs, or `scripts/e2e` change            |
| `linux_e2e`   | `Linux Tauri WebKit E2E`                                | The same, plus the Tauri E2E runner and the coverage verifier                             |
| `security`    | `Dependency security audit`                             | npm or Cargo manifests, the RustSec policy, or the audit action change                    |
| `aur`         | `aur-package`                                           | Application sources, the AUR recipe, or its packaging scripts change                      |

The contracts live in `scripts/ci/gate-relevance.mjs` and are covered by
`scripts/ci/gate-relevance.node-test.mjs`, which runs in the ungated
`Test (ubuntu-latest)` job, so the rules that decide what may be skipped are
themselves always verified.

`security` deliberately does not track application sources. The audit reads
dependency manifests and the RustSec policy, so an ordinary product change
cannot alter its result and should not re-run it.

## Fail-closed rules

Every gate runs unless the classifier can positively prove it irrelevant:

- Any non-pull-request event runs every gate.
- A missing, malformed, or unreachable base commit runs every gate.
- A classifier crash, or a decision missing any gate, runs every gate.
- An empty change set runs every gate, because a diff that describes no change
  is more likely to be wrong than to be real.
- Absolute paths, drive-letter paths, parent traversal, and empty entries are
  treated as relevant.
- An unrecognised gate name has no contract to prove irrelevance with and is
  always relevant.
- `.github/workflows/ci.yml`, everything under `scripts/ci/`, the package and
  lockfile manifests, and the Rust toolchain files run every gate, so a change
  to the relevance rules can never skip the gate it governs.

The job outputs are written in a single append after all gates are decided, so
a partially written decision can never be read as a complete one.

Renames and deletions are safe because the diff uses `--no-renames`, which
lists the removed path and the added path separately. A file renamed out of a
gated directory still triggers that gate through its original path.

## Required checks stay reported

`Build, install, launch, and uninstall Windows bundles`, `Actual Windows Tauri
dev E2E`, `Linux Tauri WebKit E2E`, and `Dependency security audit` are
required status checks with `enforce_admins` enabled. The jobs therefore always
run and always report; only their expensive steps are conditioned, and a
skipped gate records why. This intentionally avoids depending on how branch
protection treats a skipped required check, at the cost of a short job setup on
irrelevant changes.

## Periodic full verification

`ci.yml` runs on a weekly schedule in addition to pushes and pull requests.
Scheduled runs are non-pull-request events, so they exercise every gate against
`main` regardless of relevance and surface a path contract that has drifted
away from reality.
