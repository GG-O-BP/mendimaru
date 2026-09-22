# Browser testing

Mendimaru runs one declarative Playwright suite against either an explicit
HTTP(S) URL or an HTTP-ready Runtime session. The suite does not inspect the
host OS or infer a WinBoat port: Runtime URL discovery stays inside the selected
Runtime adapter.

This milestone enables `browser.test` and `browser.artifacts` on Linux
`x86_64` and `aarch64`. Windows and macOS advertise these capabilities as
unsupported until the native parity work in issue #27 is complete.

## Prerequisites and installation policy

The installed application includes the pinned Playwright JavaScript runner,
but invokes a host Node.js 22.22.2 or later executable and a separately
installed pinned Chromium build. A test never downloads a browser implicitly.

The AUR package declares Node.js as a runtime dependency and installs the runner
and its locked JavaScript dependencies under `/usr/lib/mendimaru/browser/`.
An AUR installation needs neither npm nor a Mendimaru source checkout to run
the following commands. The optional system Chromium/Chrome packages used for
Marketplace discovery do not replace the pinned Playwright browser.

```bash
mendimaru browser doctor --json
mendimaru browser install chromium --json
mendimaru browser doctor --json
```

`doctor` reports the Node.js, required minimum Node.js, Playwright, and Chromium
versions and separately reports whether Node.js is supported and Chromium is
installed and launchable. The CLI checks the runner and Node.js before invoking
JavaScript, so a missing or broken runner still produces a complete report.
`checks` always contains `runner`, `node`, `node_version`, `js_dependencies`, and
`chromium`, in that order. Each check has a `passed`, `failed`, or `skipped`
status, a safe message, and an action. Failed checks include a stable cause code:

| Cause code                | Action                                                                        |
| ------------------------- | ----------------------------------------------------------------------------- |
| `runner_missing`          | Reinstall the browser resources or correct the runner override.               |
| `runner_unreadable`       | Restore read permission on the browser resources.                             |
| `unsafe_override`         | Unset the affected override or use an absolute, direct regular file.          |
| `node_missing`            | Install Node.js and check PATH or the Node override.                          |
| `node_spawn_denied`       | Check executable permissions and filesystem execution restrictions.           |
| `node_spawn_failed`       | Install a Node.js executable compatible with the host.                        |
| `node_unsupported`        | Upgrade Node.js to 22.22.2 or later.                                          |
| `node_probe_failed`       | Check that the configured executable returns a valid Node.js version.         |
| `probe_timeout`           | Check Node.js/Chromium startup and rerun doctor.                              |
| `js_dependencies_missing` | Reinstall the locked browser dependencies; use `npm ci` in a source checkout. |
| `runner_failed`           | Inspect the private diagnostic and reinstall matching resources/dependencies. |
| `runner_output_invalid`   | Reinstall matching resources and remove stale runner overrides.               |
| `chromium_unavailable`    | Install the pinned Chromium build and check its system dependencies.          |

Runner and Node override failures are reported independently. Later checks that
cannot run are marked `skipped`; unobserved Node.js/Playwright versions are
`"unavailable"`. Both a healthy and an unhealthy inspection return one success
envelope (`ok: true`) with structured `data` on stdout and no stderr. The CLI
exits **0 when ready and 1 when not ready**; `ok` means the inspection completed,
while `data.ready` indicates toolchain readiness. Unsupported platforms and
invalid CLI arguments retain the general CLI error behavior. The internal
JavaScript runner's doctor response retains its existing shape; the host adds
the prerequisite checks and diagnostic metadata.

An unhealthy inspection retains at most one private report at
`<Mendimaru cache>/browser-tests/doctor/latest.json` (directory mode 0700, file
mode 0600 on Unix, at most 16 KiB). `diagnostic.reference` is the opaque
`browser-doctor-latest` identifier; `diagnostic.stored` reports whether saving
succeeded. An unavailable or unsafe cache does not suppress the checks. A later
failure atomically replaces this report. A healthy inspection leaves the latest
failure available for troubleshooting.

Node.js version probing is limited to 2 seconds and runner inspection to 20
seconds, with bounded process-tree cleanup afterward. Each probe captures at
most 64 KiB per output stream while draining excess output. Diagnostics retain
only allowlisted error kinds (such as `ERR_MODULE_NOT_FOUND` or `SyntaxError`),
known dependency names and truncation flags. Raw stderr, arbitrary module names,
stack traces, source excerpts, local paths and tokens are discarded, including
when JavaScript fails before it can emit JSON. Version fields are validated
before publication. No doctor check downloads resources. Only the explicit
`install chromium` command may download it. CI should install browser system
dependencies and Chromium before running the suite:

```bash
npx playwright install --with-deps chromium
```

This follows Playwright's documented
[browser installation](https://playwright.dev/docs/browsers) and
[CI setup](https://playwright.dev/docs/ci) boundaries.

## Running a suite

Exactly one target is required:

```bash
mendimaru browser test \
  --base-url http://localhost:8080/ \
  --suite-path tests/browser/smoke.browser.json \
  --fail-on-console-error \
  --fail-on-network-failure \
  --json

mendimaru browser test \
  --runtime-session-id runtime_0123456789abcdef0123456789abcdef \
  --suite-path tests/browser/smoke.browser.json \
  --json
```

The Runtime form calls the common Runtime `status` and `url` operations and
rejects a session unless `httpReady` is true. It works unchanged for Linux
Portable Runtime and Linux+WinBoat Studio Pro Run Locally forwarding.

The browser-specific controls are:

| Option                    |                          Default |  Accepted range |
| ------------------------- | -------------------------------: | --------------: |
| `--navigation-timeout-ms` |                            30000 |   100–300000 ms |
| `--action-timeout-ms`     |                            10000 |   100–300000 ms |
| `--assertion-timeout-ms`  |                             5000 |   100–300000 ms |
| `--max-artifact-mib`      |                              128 |       1–512 MiB |
| `--retention-runs`        |                               20 |      1–100 runs |
| `--asset-mirror`          |                           `auto` |          `auto` | `off` |
| `--workers`               |                                1 |     1–8 workers |
| `--worker-timeout-ms`     | none, or 600000 above one worker | 1000–1800000 ms |

`--fail-on-console-error` and `--fail-on-network-failure` promote the
corresponding diagnostics to test failures. Uncaught page errors always fail a
test. `--record-video` and `--record-har` are opt-in; HAR response/request
content is omitted.

`--asset-mirror` controls the automation-only `host.lan` asset correction for
Runtime targets (#141). It requires `--runtime-session-id`; `--base-url` never
mirrors assets. `auto` (the default) preserves the historical Studio
Run Locally behavior, while `off` starts no mirror and installs no browser
route, so Chromium runs unmodified exactly like an ordinary Linux Chrome.
Every run records what happened in `corrections` and `browserParity` (see
[Results and artifacts](#results-and-artifacts)), so an assisted pass can
never be mistaken for ordinary-Chrome parity.

`--workers` opts into bounded parallel execution of one suite inside one runner
process, and `--worker-timeout-ms` is the per-test deadline that comes with it.
`--workers 1`, the default, is the historical path exactly: one Chromium, tests
in declaration order, no per-test deadline. Asking for more workers alone
changes nothing — each test must also declare the shared resource it needs, and
the effective worker count is bounded by the host and the suite. See
[Bounded parallel test execution (#155)](#bounded-parallel-test-execution-155).

A passed suite exits `0`. An executed suite with failed assertions or policy
violations still writes its complete, schema-valid result envelope to stdout
but exits `1`. Invocation, precondition, and backend errors write one error
envelope to stderr and use the general CLI exit codes. This distinction lets an
agent consume failure evidence without parsing diagnostic text.

## Declarative suite format

### Studio session observation

On Linux, `browser test --runtime-session-id` reads Studio version metadata from
the local registered owner or the keeper's private Unix socket. It never opens
another RDP connection to discover that metadata. Missing, stopped, malformed,
incompatible, or timed-out owner metadata fails before browser execution with
retryable `precondition_failed` and a path-free diagnostic identifying the
unavailable session owner. Socket status responses have a two-second deadline.

Issue [#148](https://github.com/GG-O-BP/mendimaru/issues/148) reproduced the old
browser metadata query replacing an existing RDP connection and triggering
keeper teardown without concurrent external work. This corrects the earlier
attribution in #63/#141: external interference is not required to reproduce
that defect. Ordinary Chrome's `host.lan` asset-resolution failure is a separate
problem covered by the [opt-in generated-import repair](winboat-assets.md).

The fixture regression suite and the optional existing-session live gate are
described in the [WinBoat regression matrix](winboat-regression-matrix.md#safe-browser-and-runtime-observation-148).

### Suite structure

Suites validate against
[`browser-suite.schema.json`](../schemas/browser-suite.schema.json). They are
data, not executable JavaScript. The checked-in smoke suite is a complete
example:

```json
{
  "schemaVersion": "1.0.0",
  "name": "Mendix smoke",
  "beforeEach": [{ "action": "goto", "path": "/" }],
  "tests": [
    {
      "name": "update a widget",
      "steps": [
        {
          "action": "fill",
          "locator": { "by": "mendixName", "value": "TaskInput" },
          "value": "Review order"
        },
        {
          "action": "click",
          "locator": { "by": "role", "role": "button", "name": "Save" }
        }
      ]
    }
  ]
}
```

Prefer locators in this order:

1. `role` plus accessible `name`;
2. `label`;
3. an explicit `testId`;
4. `mendixName`, which resolves a stable `.mx-name-<value>` class;
5. visible `text` only when none of the above expresses the element.

Coordinates, viewport-dependent selectors, arbitrary CSS/XPath, and executable
callbacks are intentionally absent. These choices follow Playwright's
[locator guidance](https://playwright.dev/docs/locators) and Mendix's documented
[`mx-name` test selector](https://docs.mendix.com/howto/front-end/selenium-support/).
Supported actions are `goto`, `click`, `fill`, `check`, `uncheck`,
`selectOption`, `press`, `expectVisible`, `expectHidden`, `expectText`,
`expectValue`, and `expectUrl`. Navigation is same-origin and URL expectations
compare paths rather than platform-specific host setup.

## Authentication and secret boundary

### WinBoat UNC widget assets

For **ordinary Chrome and `browser test --base-url`**, use the explicit
[generated-import watcher](winboat-assets.md). It fixes selected-project widget
imports before Studio's Rspack bundling and continues across regenerations.
It requires `--rewrite-generated-assets`, preserves model/widget originals, and
needs no hosts/privileged-port setup or browser interception.

The older `browser test --runtime-session-id` path still starts an ephemeral
loopback-only asset mirror and installs a Chromium route for
`http(s)://host.lan/Data/**`. This is an automation-only compatibility path; it
must not be used as evidence that ordinary Chrome works. Pass
`--asset-mirror off` to disable it and run the unmodified-browser path instead;
both variants record their corrections and parity in the result (see
[Results and artifacts](#results-and-artifacts)). Requests are restricted
to `<shared-directory>/<project>/deployment/web/**`. Query-bearing paths,
non-GET/HEAD requests, traversal, symlinks, directories, and files over 64 MiB
are rejected. The mirror is destroyed with the browser run and never exposes a
LAN listener, project paths, or model files.

The mirror does not repair missing aggregate CSS or CSS imported as JavaScript by the generated
client; see [widget CSS diagnostics and the upstream reproduction](widget-css-diagnostics.md).
For #63 acceptance, exercise the normalizer with an ordinary browser or
direct-URL browser suite, without that route. The normalizer requires no
system-wide `curl http://host.lan/...` installation because Rspack bundles the
normalized imports into assets served by the normal Runtime URL.

Test credentials are never CLI arguments or suite literals. A suite may read
only environment variables named `MENDIMARU_TEST_<NAME>`:

```json
{
  "action": "fill",
  "locator": { "by": "label", "value": "Password" },
  "valueFromEnv": "MENDIMARU_TEST_PASSWORD"
}
```

`valueFromEnv` is sensitive by default. Use `sensitive: false` only for a
non-secret fixture value. `storageStateEnv` may name an environment variable
whose value is the absolute path of a direct, bounded Playwright storage-state
JSON file. Cookie and local-storage values from that file are treated as
secrets. `secretEnv` declares any additional values that must be removed.

Before capture, password fields, `[data-mendimaru-private="true"]`,
`.mx-name-MendimaruPrivate`, and `maskLocators` are visually masked. Textual
artifacts and trace entries are redacted. Publication then independently scans
every file and every ZIP member for raw, percent-encoded, and Base64 forms of
all declared secrets. The scan uses a 64 KiB streaming buffer and preserves
enough overlap to detect a value split across read boundaries. Any hit fails
publication. Suite and storage-state symlinks are rejected, and the run
directory/files use user-only permissions on Unix.

Artifact scanning has fail-closed safety ceilings that are independent of the
user-configurable compressed inventory limit:

| Safety ceiling                   | Limit                   |
| -------------------------------- | ----------------------- |
| Regular artifact actual bytes    | 512 MiB                 |
| ZIP members                      | 4,096                   |
| ZIP central directory            | 8 MiB                   |
| One ZIP member, uncompressed     | 64 MiB                  |
| One ZIP, cumulative uncompressed | 256 MiB                 |
| ZIP member compression ratio     | 200:1                   |
| Whole publication scan wall time | 30 seconds              |
| ZIP member name / path depth     | 1,024 B / 32 components |

ZIP central-directory declarations are checked before decompression. Only
unencrypted Stored and Deflate members with bounded, relative paths are
accepted; symlink and special entries are rejected. Declared limits are checked
again against bytes actually produced while streaming, and size mismatches,
truncation, malformed headers, unsupported encryption/compression, and CRC
errors all abort publication. These ceilings cannot be raised with
`--max-artifact-mib`.

## Results and artifacts

The result validates against
[`browser.schema.json#/$defs/summary`](../schemas/browser.schema.json). It
contains per-test outcomes and step counts, timestamps, browser/Playwright
versions, and content-addressed artifact descriptors. Every summary and the
artifact manifest also record the automation-only corrections the runner
did or did not apply (#141): a `corrections` array with one entry per kind —
today `host-lan-asset-mirror` with `applied` and the number of
`interceptedRequests` it rewrote — plus a top-level `browserParity` of
`unmodified` or `assisted`. `assisted` means the runner modified the browser
context (for example by routing `host.lan`), so that pass is **not** evidence
that ordinary Chrome works; only `unmodified` runs are. The Rust validator
rejects a summary whose corrections disagree with the requested mirror or whose
parity disagrees with its corrections, and the HTML report shows the parity and
correction counts next to the outcome. The manifest also records host, Studio,
Runtime, backend/mode, available Studio/Runtime versions, suite metadata, and
the effective policy without recording the base URL, query data, suite path,
or credentials.

Every run stores machine-readable `summary.json`, diagnostic JSON, an artifact
manifest, and a human-readable `report.html`. Failed tests additionally store a
masked screenshot, DOM snapshot, accessibility tree when available, and a
Playwright trace; Playwright documents the trace contents and viewer in its
[Trace Viewer guide](https://playwright.dev/docs/trace-viewer-intro). Optional
video and HAR files follow the same size and retention policy.

```bash
mendimaru browser artifacts \
  --session-id session_0123456789abcdef0123456789abcdef \
  --json
```

On Linux the private files are under
`${XDG_CACHE_HOME:-$HOME/.cache}/com.ggobp.mendimaru/browser-tests/runs/<sessionId>/`.
`browser artifacts` re-hashes and re-sizes every file before returning its
descriptor; a modified or symlinked file is rejected. The current run is never
deleted during its own commit. Older runs are pruned best-effort by count and a
1 GiB global cap, while each run is independently limited by
`--max-artifact-mib`. A scan or validation failure occurs before `index.json`
and the atomic run-directory rename, so the staging directory is removed and
the failed session does not appear in `browser artifacts`.

## Verification

The repository test uses real headless Chromium, not a mocked browser:

```bash
npm run test:browser
```

It exercises the same platform-neutral suite for Portable and WinBoat metadata,
login and Mendix widget interaction, assertion/navigation/page failures,
console/network policy behavior, optional video/HAR, unavailable Chromium
diagnostics, failure evidence, and whole-artifact secret scanning. A malicious
local target also produces a small, highly compressible trace with a member
over the uncompressed limit; the test verifies pre-extraction rejection, a
576 MiB runner RSS budget, and a successful next run. The Rust CLI E2E launches
the compiled executable and runs that exact smoke suite through both a real
Portable supervisor URL and a WinBoat loopback adapter URL. It also verifies
exit/stream semantics and Runtime readiness rejection, re-queries artifacts,
checks integrity, and scans trace members again.

## Mirror correction parity (#141)

A dedicated Rust CLI E2E regression test runs the **same fake-WinBoat fixture
and the same suite twice** against one Studio Run Locally Runtime: once with
the default mirror and once with `--asset-mirror off`. The assisted run must
pass with `browserParity=assisted`, `applied=true`, and at least one
intercepted request, while the unmodified run must fail on the ordinary
`host.lan` path with `browserParity=unmodified` and zero interceptions. Both
runs stay retained as separate sessions with their own artifacts, so a mirror
assisted pass can never silently stand in for the ordinary-browser result.

The same fixture isolation cannot prove the real user path, so the release
workflow also runs a self-hosted **Studio F5 → Linux Chrome parity gate** on a
runner labeled `linux, winboat-studio` before a GitHub release is created. It
builds the Linux release binary, starts a real shared-project Studio Run
Locally Runtime, and requires the suite to pass with `--asset-mirror off` —
unmodified Chromium, no fake fixture, no Portable Runtime substitute. An
assisted (`--asset-mirror auto`) run is executed afterwards purely as
comparison evidence and can never satisfy the gate. The gate script is
`scripts/e2e/studio-f5-parity-gate.mjs` and is driven by:

- `MENDIMARU_STUDIO_PARITY_BINARY` — absolute path to the release binary,
- `MENDIMARU_STUDIO_PARITY_SUITE` — absolute path to the parity suite,
- `MENDIMARU_STUDIO_PARITY_EVIDENCE` — evidence JSON output path (default
  `artifacts/e2e/studio-f5-parity.json`).

Because widget-usability claims need representative state changes, the gate
rejects pure-navigation suites (`goto`/`expectVisible` only): the suite must
contain at least one state-changing action (`fill`, `click`, `selectOption`,
`press`, `check`, `uncheck`) and one value assertion (`expectText`,
`expectValue`) — for example a cell edit followed by the computed value.
Write-back and file import/export coverage follow the same rule: assert the
observed state, not the absence of runner errors.

## Opt-in frontend health (#144)

`browser frontend-health` opens the target in a fresh Chromium session with
ordinary network resolution. Operations → Frontend health exposes the same
application operation in English, Korean and Japanese. Nothing runs until the
user explicitly requests a diagnosis. The existing `browser.test` platform
capability and pinned, explicitly installed toolchain apply.

```bash
mendimaru browser frontend-health --base-url http://localhost:8080/ --json
mendimaru browser frontend-health \
  --runtime-session-id runtime_0123456789abcdef0123456789abcdef \
  --navigation-timeout-ms 15000 --observation-ms 3000 --json
```

WinBoat Runtime targets hold shared VM use through the complete diagnosis.
For a plain URL in the configured VM, add `--winboat-use`; a plain URL without
this flag remains external. In the desktop, use the Runtime session ID for VM
protection. The lease does not enable asset bypass or RDP discovery. See
[shared VM use](winboat-vm-use.md).

Exactly one target is required. Navigation accepts 100–30000 ms (default 15000),
and observation accepts 100–10000 ms (default 3000). The desktop uses these
defaults. The runner has a separate bounded process-tree supervisor, 128 KiB
output capture, and startup/cleanup allowance. Diagnosis never downloads tools.
Run `browser doctor --json` if prerequisites fail.

Three results are kept separate:

| Field           | Meaning                                                                                                                                                                                          |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `studioState`   | Session-owner-reported Studio process state for a linked Runtime; null for an explicit URL/Portable Runtime. It does not prove frontend readiness.                                               |
| `httpReady`     | The existing lightweight Runtime readiness snapshot, or final document HTTP status `<500` for an explicit URL. Null when no HTTP response was observed. Even HTTP 404 can satisfy this contract. |
| `frontendState` | `healthy` means no failure was observed during this fresh-session window; `unhealthy` means a browser failure was observed; `inconclusive` means navigation/loading/observation was incomplete.  |

An HTTP 200 page may be `unhealthy` with `httpReady: true`. Blank content,
unfinished document/script/stylesheet requests, observation timeout and truncated
evidence cannot yield `healthy`. This is a bounded startup observation, not proof
of all app functionality. It does not log in, share an ordinary browser's
cookies, exercise app actions, or assert a project-specific ready element. A
working login page may be healthy. Use a declarative suite for authenticated or
app-specific assertions and rerun diagnosis after changes. Reports are snapshots,
not a persistent frontend-ready flag.

The observer collects uncaught page errors, console errors (including ESM load
errors), failed network requests, HTTP errors, visible Mendix error dialogs and
native browser dialogs. Native dialogs are recorded and dismissed to allow the
observation to finish. DOM error dialogs are observed without clicking them.
It uses Playwright's [request/response events](https://playwright.dev/docs/api/class-page#page-event-requestfailed)
and [dialog handling](https://playwright.dev/docs/dialogs); HTTP 4xx/5xx responses
are recorded separately because they are not failed network requests.

Results validate against [frontend-health.schema.json](../schemas/frontend-health.schema.json).
A completed observation emits one success envelope on stdout, no stderr, with
exit **0 only for `healthy`** and **1 for `unhealthy`/`inconclusive`**. Invalid
arguments, unsupported platforms, unavailable tools and an HTTP-unready Runtime
retain normal CLI error envelopes; no frontend health is claimed for them.

Diagnostics contain stable `code`, actionable `action`, bounded `occurrences`,
and optional `endpoint`, network `failure`, and HTTP `status`. Endpoints classify
scheme, host (`shared-unc`, `same-origin`, `loopback`, `external`, `unknown`), port
and path (`shared-deployment`, `stylesheet`, `script`, `document`, `other`). They
deliberately omit raw hosts, paths, query strings, credentials and console/error
text. `shared-unc` identifies `host.lan/Data/…`; its failures are listed first,
with the underlying DNS/connection/TLS/HTTP cause retained. Guidance points to
the explicit [generated-import repair](winboat-assets.md), without claiming a
widget defect or applying that repair. Aggregate widget CSS 404 has separate
[CSS guidance](widget-css-diagnostics.md). There are at most 100 distinct
findings, 10000 occurrences per finding and 10000 events per counter. Overflow
sets `truncated`; the report contains no screenshots, DOM snapshots or raw logs.

Unlike `browser test --runtime-session-id`, this operation never starts an asset
mirror or installs browser routes. `assetBypass` is always false. It does not
modify projects, generated imports, Compose, hosts files;
it does not connect to RDP or discover Studio through guest automation. Runtime
URL and Studio state come from the existing safe status read. Existing
`runtime status`, `wait`, `url` and lifecycle behavior remain unchanged.

`npm run test:browser:frontend` exercises real headless Chromium on HTTP fixtures
for HTTP 200 with ESM/CSS/dialog failures, shared UNC failures without a mirror,
404/503, blank pages, delayed dialogs, native dialogs, unfinished requests,
timeouts, event floods and privacy. The ordinary Rust suite runs those same
pages through the compiled CLI, validates Runtime observation against a real
keeper socket and checks zero RDP launches or Compose recreations. These are
controlled fixtures, not a new live Windows VM acceptance claim.

## Shared WinBoat use

WinBoat Runtime targets automatically hold shared VM use for the complete browser
command. For a plain URL in the configured VM, add `--winboat-use`; separate
config/cache directories still coordinate through the same management identity.
Lifecycle changes return a bounded, retryable busy precondition while tests hold
use. See [VM use policy and boundaries](winboat-vm-use.md) for modes, timeout and
cancellation, generations, and the advisory trust boundary.

## Shared test sessions (#151)

`browser session prepare` records a readiness-verified Runtime identity once;
`browser test --shared-session-id` then joins it. Workers inherit the recorded
Runtime/Studio identity and stabilized ready state instead of re-running
discovery, so attaching opens no metadata path and no RDP connection. Each
worker's success, failure, cancellation, or crash cleans only its own browser,
context, artifacts, and participation — never the Studio session, keeper, or
VM. The last participant leaving triggers nothing by itself: cleanup happens
only when the owner runs `browser session finalize`, which waits bounded for
live participants and applies the recorded policy exactly once. A stop at
finalize requires the explicit prepare-time `--owns-runtime` claim; sessions
that attached to a user-started Runtime keep it running. Participation
liveness is kernel lock ownership, not a reference count or RDP state. See
[headless CLI](headless-cli.md#shared-browser-test-sessions-151) for the full
state, recovery, and race contract.

## Browser environment change evidence (#154)

WinBoat browser runs now record bounded environment observations and interrupt on
changes. See [environment generations](browser-environment-observation.md) for
`--build-marker`, JSON comparability, preparation boundaries, observation limits,
and the separate external-change fixture and disposable-VM gates.

## Bounded parallel test execution (#155)

`--workers N` (1–8, default 1) runs at most N tests of one suite at a time
inside **one** runner process against **one** prepared Runtime.
`--worker-timeout-ms MS` (1000–1800000) is the per-test deadline. Left unset it
means no per-test deadline with one worker and 600000 with more than one; an
explicit value always applies. One worker reproduces the historical behavior
exactly — one Chromium, declaration order, no deadline — so an existing
invocation is unchanged.

Parallelism is opt-in twice. The operator asks for workers, **and** the suite
declares, per test, which shared resource that test needs:

```json
{
  "name": "read the order list",
  "concurrency": { "resource": "app-read" },
  "steps": []
}
```

```json
{
  "name": "edit a cell in the isolated order set",
  "concurrency": {
    "resource": "data-write",
    "scope": "orders-a",
    "isolation": "verified"
  },
  "steps": []
}
```

| Field       | Values                                                | Rules                                                                                                                          |
| ----------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `resource`  | `app-read`, `data-write`, `studio-ui`, `vm-lifecycle` | Required whenever `concurrency` is present.                                                                                    |
| `scope`     | `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`                   | Optional, and accepted only for `data-write` and `studio-ui`.                                                                  |
| `isolation` | `verified`, `unverified` (default)                    | `verified` is accepted only for `data-write` and only together with a `scope`; `verified` without a scope is an invalid suite. |

A test **without** a `concurrency` declaration is never assumed parallel-safe:
it joins the serial `data-write` group. An existing suite therefore gains no
unsafe overlap when workers are requested. Unknown keys and unknown values in a
declaration are refused as an invalid suite before any browser starts; the host
checks the declarations itself and does not rely on the runner to catch them.

### Parallel permission table

| Declared resource                                                                                         | Runs beside                                                       | Excluded from                                                                                                                      |
| --------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `app-read` — stable app observation                                                                       | other `app-read` tests, scoped verified `data-write`, `studio-ui` | any `data-write` whose isolation is not verified                                                                                   |
| `data-write` with `isolation: verified` and a `scope`                                                     | readers and other scopes                                          | another test in the same scope, any unverified write, `vm-lifecycle`                                                               |
| `data-write` without verified isolation — write-back and import/export whose data isolation is not proven | nothing                                                           | every reader, every other writer, every Studio UI action                                                                           |
| `studio-ui`                                                                                               | readers and scoped verified writes                                | any other Studio UI action, in any scope — one in-flight guest request per Studio, the [#152](winboat-ui-coordination.md) boundary |
| `vm-lifecycle`                                                                                            | nothing; it runs alone                                            | everything, and it is refused outright for a shared-session participant                                                            |

An exclusive `vm-lifecycle` test is a barrier. Admission is head-of-line in
declaration order and later tests never overtake it, so a stream of short
readers cannot starve it. `browser test --shared-session-id` refuses a
`vm-lifecycle` test outright, before any browser starts
(`concurrency_policy_refused` in the runner): an attached worker never starts
or stops the Runtime, because the owner prepares and finalizes
([#151](#shared-test-sessions-151)).

### Determinism, isolation, and cancellation

- Test results, the artifact inventory, console/page-error/network diagnostics,
  and the report schema are assembled in **suite declaration order**, not
  completion order. Only the schedule varies.
- Each test keeps its own browser context and its own `test-NNN…` artifact
  names, and each lane has its own Chromium process. One lane's crash, timeout,
  or cancellation ends only that lane's entry: another lane's browser, context,
  and artifacts survive it, and it never touches the VM, Studio, or Runtime.
- A per-test deadline aborts only that test. An environment generation change
  ([#154](browser-environment-observation.md)) cancels the in-flight tests and
  skips the pending ones; the runner observes the change while every lane is
  busy instead of waiting for the slowest lane.
- Every affected test records `invalidatedBy`: `environment-change`, `timeout`,
  or `cancelled`. An interrupted test that never started stays `skipped` with
  `invalidatedBy: "environment-change"`; a cancelled in-flight test is
  `failed`. Neither is ever recorded as a pass, and the HTML report prefixes
  the failure text with that reason.
- A lane that cannot produce a result at all — a Chromium that fails to launch
  for that lane, for example — records one explicit `failed` test, not a silent
  pass and not a whole-run abort.

### Reported concurrency evidence

Every summary, the artifact manifest, and the HTML report carry a `concurrency`
record:

| Field                     | Meaning                                                                                                                                                |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `requestedWorkers`        | The requested `--workers` value.                                                                                                                       |
| `effectiveWorkers`        | The lanes the run actually used.                                                                                                                       |
| `limitedBy`               | `request`, `cpu`, `memory`, or `suite` — the bound that produced `effectiveWorkers`.                                                                   |
| `maxObservedParallel`     | The measured peak number of tests in flight at once, not the permitted number.                                                                         |
| `testTimeoutMilliseconds` | The per-test deadline; omitted when none applies.                                                                                                      |
| `sessionRole`             | `owner` or `participant`.                                                                                                                              |
| `groups`                  | The declared permission table: `resource`, `mode` (`parallel`, `scoped-parallel`, `serial`, `exclusive`), `tests`, and `scopes` for `scoped-parallel`. |

The runner lowers the requested count to the host CPU count
(`os.availableParallelism()`), to a memory budget of 768 MiB per lane, and to
what the suite could actually overlap — one lane per `app-read` test, per
distinct verified `data-write` scope, and one Studio UI turn, capped by the
test count. The bounds are applied in the order request → CPU → memory → suite,
and `limitedBy` names the last one that lowered the count. Asking for workers
is therefore never a promise of parallelism: a suite of undeclared tests
reports `effectiveWorkers: 1`, with `limitedBy: "suite"` unless a tighter host
bound applied first.

### Mirror corrections under parallel execution (#141)

`corrections` and `browserParity` keep their existing meaning. `assisted` still
means the runner modified the browser context, so such a run is still not
evidence that ordinary Chrome works; the `host-lan-asset-mirror` interception
count remains a run total across lanes rather than a per-test count. An
assisted run and an unmodified (`--asset-mirror off`) run stay **separate runs
with separate results and artifacts** and must not be merged into one
general-Chrome compatibility claim, and both runs must report the same build
identity for the comparison to mean anything.

### Limits and what is not proven

- Parallelism lives inside one runner process against one prepared Runtime.
  Starting several CLI processes does not make the shared VM lifecycle, the
  shared build, or the shared server data safe. The safe multi-caller paths
  remain [shared VM use](winboat-vm-use.md) (#150/#21),
  [shared test sessions](#shared-test-sessions-151) (#151),
  [Runtime port ownership](winboat-multi-runtime.md) (#153), and
  [Studio UI job coordination](winboat-ui-coordination.md) (#152).
- The scheduler enforces the declared policy; it cannot verify that a
  declaration is true. `isolation: verified` is the suite author's claim,
  backed by the named scope. A false claim is scheduled as written.
- Fixture and Portable regressions do not substitute for the real Linux+WinBoat
  gates, which keep their existing opt-in and disposable-snapshot conditions.
  See the [#155 regressions](winboat-regression-matrix.md#bounded-parallel-test-execution-155).
