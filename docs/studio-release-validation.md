# Generic Studio F5 release validation

The release gate accepts any configured Mendix application. It requires no
particular project name, widget, model filename, or fixed Studio version.
The operator selects a disposable application and a browser suite that exercises
its actual UI. The same requirements apply to every release.

## Runner configuration

Use a Linux runner with the `winboat-studio` label, an accessible WinBoat
installation, Rust/Cargo, Docker or Podman as configured by Mendimaru, FreeRDP,
Python 3, `xvfb-run`, and `xdotool`. The workflow supplies Node and frontend
dependencies. The runner account must have access to its configured VM and
shared workspace. Its Mendimaru configuration is normally
`~/.config/com.ggobp.mendimaru/config.json`; `MENDIMARU_CONFIG_DIR` can select a
different configuration directory. The VM uses that configuration's container
name; it does not need a particular name.

Configure these repository Actions variables:

| Variable                        | Meaning                                                                                                                                                      |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `STUDIO_F5_PARITY_PROJECT_PATH` | Required absolute path to the selected `.mpr` file **on the runner**, inside its configured shared directory.                                                |
| `STUDIO_F5_PARITY_SUITE`        | Required absolute path to that application's browser-suite JSON file on the runner.                                                                          |
| `STUDIO_F5_PARITY_VERSION`      | Optional exact expected Studio version. Leave empty to use the project's discovered `requiredVersion`. A mismatch fails; it never silently upgrades the app. |

The gate derives the same canonical-path project ID as `project list`, so
applications with the same name remain distinct. The selected project must
appear in discovery with an exact required version, and that version must be
installed in the guest. There is no fallback to another application or version.
Missing files, invalid version syntax, an invalid suite, or a project outside the
shared directory fail before starting a VM session. Discovery and installed
version checks run after the configured guest becomes available.

Prepare the application with official Mendix tools and the supported Studio UI.
Finish sign-in, initial setup, conversion, and update dialogs before unattended
validation. The gate does not click version-specific screen coordinates or
assume the language or appearance of a login screen. No other Studio or Runtime
owner may be active in this runner's validation environment.

## Preflight

The manual **Studio F5 parity gate self-test** workflow checks the runner tools,
configured container-runtime access, configured paths, shared-directory
containment, and suite shape.
It does not launch or stop Studio or Runtime. The same check can run locally:

```bash
export MENDIMARU_STUDIO_PARITY_PROJECT_PATH=/absolute/shared/Example/Example.mpr
export MENDIMARU_STUDIO_PARITY_SUITE=/absolute/private/app-acceptance.browser.json
# Optional expectation, for example when updating a runner:
# export MENDIMARU_STUDIO_PARITY_VERSION=11.12.6
python3 scripts/e2e/prepare-studio-f5-release.py --check-fixture
```

A suite must contain a real state-changing action (`fill`, `click`,
`selectOption`, `press`, `check`, or `uncheck`) and a value assertion
(`expectText` or `expectValue`). Choose selectors and expected values for the
selected application; no third-party component is assumed. Declare the suite's
data effects and isolation as described in [browser testing](browser-testing.md).
Use disposable data because both ordinary and assisted comparison runs may
change the application's state. Credentials belong in supported private setup
paths, not repository variables or committed suites.

## Release execution and evidence

The workflow verifies the immutable release tag and builds its Linux binary.
It prepares owned Runtime forwarding, launches the selected project with its
exact installed Studio version, locates that version's unambiguous project
window, and warms generated bundler inputs with F5 in that same owned Studio
session. A cold UNC build can fail before the generated-asset watcher repairs
its inputs; Studio retains that first failure even after Rspack rebuilds.
The gate waits at least 75 seconds and for stable generated files, then delivers
the ordinary F5 used for validation. Warm-up alone never passes the gate.
A missing or ambiguous window fails;
login or conversion dialogs are not dismissed automatically.

The gate still requires HTTP readiness from that Studio-linked Runtime, stable
generated build files, a comparable shared browser preparation, and a successful
ordinary Chromium run with `--asset-mirror off`. The assisted comparison cannot
replace the ordinary-browser result. Fake orchestration tests, portable Java
startup, and listener readiness do not count as a live gate pass.

Evidence remains at `artifacts/e2e/studio-f5-parity.json`. Browser failure or
cancellation never authorizes stopping an unrelated session. The release owner
finalizes its shared preparation and cleans up only its own Studio, Runtime,
and generated-asset watcher. Release publication continues only after this gate
and the Windows and AUR verification jobs succeed.

Validation tools come from the workflow revision, separately from the immutable
application tag, as with Windows release checks. To apply an orchestration fix
to an existing tag, dispatch the release workflow from the reviewed revision;
the application source and Linux binary still come from the unchanged tag.

Historical reports under `docs/issue-*-evidence*` describe the applications and
versions used at the time. They do not define release prerequisites. Retired
application-specific spike drivers are not part of the generic release path.
