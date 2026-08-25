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

```bash
mendimaru browser doctor --json
mendimaru browser install chromium --json
mendimaru browser doctor --json
```

`doctor` reports the Node.js, required minimum Node.js, Playwright, and Chromium
versions and separately reports whether Node.js is supported and Chromium is
installed and launchable. Only the explicit
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
  --base-url http://127.0.0.1:8080/ \
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

| Option                    | Default | Accepted range |
| ------------------------- | ------: | -------------: |
| `--navigation-timeout-ms` |   30000 |  100–300000 ms |
| `--action-timeout-ms`     |   10000 |  100–300000 ms |
| `--assertion-timeout-ms`  |    5000 |  100–300000 ms |
| `--max-artifact-mib`      |     128 |      1–512 MiB |
| `--retention-runs`        |      20 |     1–100 runs |

`--fail-on-console-error` and `--fail-on-network-failure` promote the
corresponding diagnostics to test failures. Uncaught page errors always fail a
test. `--record-video` and `--record-har` are opt-in; HAR response/request
content is omitted.

A passed suite exits `0`. An executed suite with failed assertions or policy
violations still writes its complete, schema-valid result envelope to stdout
but exits `1`. Invocation, precondition, and backend errors write one error
envelope to stderr and use the general CLI exit codes. This distinction lets an
agent consume failure evidence without parsing diagnostic text.

## Declarative suite format

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
versions, and content-addressed artifact descriptors. The artifact manifest
also records host, Studio, Runtime, backend/mode, available Studio/Runtime
versions, suite metadata, and the effective policy without recording the base
URL, query data, suite path, or credentials.

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
