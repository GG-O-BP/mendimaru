# Platform backend and capability contract

Mendimaru uses one versioned contract for Studio Pro, Runtime, UI automation,
and browser operations. The contract describes equivalent behavior; it does not
require adapters to share an implementation or transport.

The Rust contract types are in `src-tauri/src/contracts.rs`. Adapter traits and
selection live in `src-tauri/src/platform/backend.rs`. JSON Schema documents are
in `schemas/`.

Adapters own their platform configuration. Public trait methods receive only
operation data such as a version, host installer path, session ID, or portable
request DTO; they never require the legacy WinBoat-filled `AppConfig` shape.

## Capability discovery

Capability discovery does not initialize Tauri or require a GUI:

```bash
mendimaru capabilities --json
mendimaru capabilities --json --backend linux-winboat
```

Successful output is one JSON document on stdout. Diagnostics are not mixed
into stdout. A failure is one JSON document on stderr and uses these exit codes:

| Exit code | Meaning                                         |
| --------- | ----------------------------------------------- |
| `0`       | Capability snapshot returned                    |
| `1`       | Operation or precondition failed                |
| `2`       | Invalid command or argument                     |
| `3`       | Requested backend is unavailable or unsupported |

The Tauri command `get_capabilities` returns the same `CapabilitySnapshot` data
object. The CLI wraps it with the host platform, backend, invocation session ID,
and the same immutable snapshot used by every other headless command. Every
snapshot has a cryptographically random ID and capture time so a session can
retain the exact capabilities it observed when it was created. The complete
headless surface and stream contract are documented in
[`headless-cli.md`](headless-cli.md).

## Platform identity

Do not infer one identity from another:

| Backend          | `hostPlatform` | `studioPlatform` | `runtimePlatform` | `runtimeModes`                   |
| ---------------- | -------------- | ---------------- | ----------------- | -------------------------------- |
| `linux-winboat`  | `linux`        | `windows`        | `linux`           | `portable`, `studio-run-locally` |
| `windows-native` | `windows`      | `windows`        | `windows`         | `portable`                       |
| `mac-native`     | `macos`        | `macos`          | absent            | absent                           |

`runtimePlatform`, singular `runtimeMode`, and `runtimeModes` are optional and
independent. The singular field is emitted only when exactly one mode is
supported; `runtimeModes` is the authoritative advertised set. A common
request, session, error, or artifact must not require RDP, UNC, Compose,
Windows registry, app-bundle, or TCC fields.

## Backend selection

Automatic selection has exactly one mapping per supported host:

- Linux selects `linux-winboat`.
- Windows selects `windows-native`.
- macOS selects `mac-native`.

An explicit override must equal the backend mapped to the current host. A
different backend returns `backend_mismatch`; Mendimaru never silently retries
another backend, another Studio version, or another Runtime mode.

## Capability entries

Every manifest contains exactly one entry for every action in these contracts:

- `StudioBackend`: `detect`, `install`, `uninstall`, `start`, `status`, `stop`
- `RuntimeBackend`: `build`, `start`, `status`, `wait`, `url`, `stop`, `logs`
- `UiAutomationBackend`: `capabilities`, `tree`, `find`, `action`, `wait`,
  `screenshot`
- `BrowserBackend`: `test`, `artifacts`

An entry contains:

- a stable dotted capability ID;
- `supported` or `unsupported` status;
- all permissions or interactive preconditions known before invocation;
- an explicit `fallbackAllowed` value (currently always `false`);
- a structured limitation for unsupported actions, including a stable code and
  optional required permission or version.

Callers decide which actions to offer by capability ID and status. They must not
guess support from an OS name, translated error text, screen coordinates, or an
executable that happens to exist. A supported action may still return a
`precondition_failed` error when a declared permission or service is not
currently available.

The current Linux+WinBoat and Windows native adapters support Studio `detect`,
`install`, `uninstall`, `start`, `status`, and `stop`, plus Portable Runtime
`build`, `start`, `wait`, `url`, `stop`, and `logs`. Linux also supports the
WinBoat `studio-run-locally` Runtime adapter with transactional loopback port
forwarding. Session identity binds an
exact process ID to its process start time; adapters also verify the current
Windows user and the executable path of the detected installation before a
session can be shown or closed. Runtime versions are exact-policy gated and the
manifest records all supported Runtime modes independently of the Windows
Studio backend.
UI automation remains explicitly unsupported. Linux `x86_64` and `aarch64`
support browser test execution and integrity-checked artifact lookup for both
Portable and WinBoat Runtime URLs. Windows/macOS browser execution remains
explicitly unsupported until issue #27 validates native parity. `mac-native` is
registered but otherwise reports operations as unsupported until issue #11
supplies the native implementation.

## Errors, sessions, and artifacts

`BackendError` always contains `schemaVersion`, a machine-readable `code`, a
human message, and `retryable`. Backend and capability IDs are present when the
error belongs to an adapter operation. `unsupported_capability` includes the
same structured limitation represented by capability discovery.

`SessionDescriptor` embeds an immutable `CapabilitySnapshot`; later environment
changes do not rewrite what the caller was told at session creation. Session
and snapshot IDs use 128 bits of operating-system randomness.

`StudioSessionStatus` is the successful Studio `status` payload. It includes
the Studio version, process identity, optional start time and project basename,
connection state, and a machine-readable reconnect availability reason. No
project path or Windows command line crosses the adapter boundary. Before this
surface was implemented, both production adapters reported `studio.status` and
`studio.stop` as unsupported and emitted no successful status payload, so its
initial supported shape was introduced in `1.0.0` and is unchanged in `2.0.0`.

Linux GUI project launches classify their internal selection as either
`ConfiguredWorkspace` or `ExplicitHostSelection`. The WinBoat adapter resolves
that selection through a `ProjectAccessProvider` before spawning Studio. Every
provider returns the same lease semantics: a guest project path, provider kind,
bounded non-secret share identity when applicable, cleanup ownership, and
reconnect policy. The first external provider adds one FreeRDP drive redirect
to the same retained RemoteApp connection; the common application, launch
assistant, operation record, and session DTO do not contain UNC or drive
details. A future capability-advertised WinBoat provider can implement the same
lease contract without changing those callers, and provider failures are not
silently downgraded after side effects begin. The GUI selection and launch
reference use a bounded current-process digest token instead of serializing the
raw external host path.

`ArtifactDescriptor` links every artifact to a session and backend. Local
location, media type, digest, size, and backend diagnostic reference are
optional. This keeps platform paths out of the portable required shape while
allowing an adapter to provide a verified local artifact.

Version `3.0.0` introduces the supported browser result and manifest shapes,
including per-test outcomes, browser/Playwright versions, and verified artifact
descriptors. The optional Runtime `runtimeVersion` is carried into a browser
manifest when the adapter can establish it. See
[`browser-testing.md`](browser-testing.md).

Version `4.0.0` adds stable external-process timeout, cancellation, and
interruption error codes so callers can distinguish bounded process failures
without parsing localized messages.

`RuntimeStatus` is the shared successful lifecycle payload on Linux and
Windows. Version `2.0.0` adds the backend identity and an explicit `httpReady`
gate. WinBoat sessions may also expose optional host/guest ports, linked Studio
identity, and Studio state; these are not required for native or Portable
adapters. Deployment paths, admin ports, credentials, Compose paths, and host
launcher details remain private. See
[`portable-runtime.md`](portable-runtime.md) and
[`winboat-run-locally.md`](winboat-run-locally.md).

## Adding an adapter

1. Add a stable `BackendId` and its exact host/studio platform mapping.
2. Implement `BackendIdentity` and all four backend traits. Leave an operation
   on the default implementation only when its manifest marks it unsupported.
3. Put platform transport details in the adapter or its private implementation;
   do not add them as required common fields.
4. Add permissions, version limitations, and support status for every
   `CapabilityId`. Do not omit entries.
5. Extend the selection matrix. Reject host/backend mismatches instead of
   falling back.
6. Run the same fake-adapter contract suite and add adapter integration tests
   for every capability marked supported.
7. Update the JSON Schemas and compatibility notes when the serialized contract
   changes.

## Compatibility and versioning

The current schema version is `4.0.0` and follows semantic versioning. Version
2 added multi-mode capability discovery, explicit Runtime backend/readiness,
WinBoat Run Locally status fields, and its distinct failure codes. Version 3
adds the first supported browser-test contract and optional Runtime version
metadata. Version 4 adds external-process failure enum values. These are major
versions because they add serialized fields or enum values and, for version 3,
give the previously unsupported browser result a complete shape:

- Patch: documentation or validation corrections that do not change accepted
  data or meaning.
- Minor: additions already accepted by the current schemas, such as another
  permission string or optional artifact metadata value with unchanged meaning.
- Major: added, removed, or renamed serialized fields; new or removed enum
  values and capability IDs; newly required values; or changed semantics.

Readers must reject an unsupported major version and may reject an unsupported
minor version when its closed schema cannot validate the document. Writers emit
only the version they implement. Capability snapshots and artifacts retain
their original schema version rather than being rewritten in place.

The GUI's existing `EnvironmentStatus` still contains WinBoat diagnostic fields
for presentation compatibility. It is a legacy UI DTO, not the portable CLI or
agent contract.

## Verification

`npm run check:portable` runs lint, format, frontend tests, Rust
contract/fake-adapter tests, real CLI process tests, and JSON Schema validation. `npm run
test:browser` additionally runs the real Chromium policy and artifact suite.
On Linux, `npm run test:e2e` separately launches the real Tauri debug binary,
Vite development server, and WebKit WebView through `tauri-driver`, backed by
isolated WinBoat/API/project fixtures. It observes motion on an actual React
busy state, samples multiple frames of the online route marker, and then
requires every idle animation except that explicitly allowlisted route motion
to be stopped. The React-only mocked application flow is available as `npm run
test:app-flow`; it is not a substitute for the native window gate.

The non-destructive live session suite can run against an existing, online
WinBoat VM:

```bash
npm run test:winboat-smoke
```

It queries current-user sessions through the authenticated RemoteApp transport
and verifies that reconnect and close both reject a stale PID/start-time pair.

On Linux, `npm run check` appends the destructive full adapter lifecycle so the
live Studio Pro boundary is not silently excluded from the exhaustive local
gate. It requires a disposable, currently absent WinBoat test version with an
official installer already in the host-private application cache:

```bash
MENDIMARU_E2E_ALLOW_MUTATION=1 \
MENDIMARU_E2E_VERSION=11.13.0 \
npm run check
```

The same environment variables with `npm run test:winboat-e2e` run only the
live lifecycle. On non-Linux hosts the WinBoat lifecycle reports itself as not
applicable.

The test refuses a preinstalled target rather than deleting user state. It
installs through the common adapter, verifies progress ordering and exact-version
detection, observes a real Studio window, rejects removal while running, closes
the exact authenticated process through its existing RemoteApp connection,
uninstalls it officially, and verifies that it remains absent under repeated and
post-delete operations. It also verifies that pre-existing installations and the
private installer cache are unchanged and that the temporary shared installer
staging file is removed. Both live gates run FreeRDP under Xvfb and an
isolated window manager, failing on leaked child processes or any unexpected
RAIL/PowerShell window. The host must provide `xvfb-run`, `xfwm4`, and `wmctrl`;
Arch Linux provides `xvfb-run` in `xorg-server-xvfb`.

Hosted CI has no live WinBoat VM, so it runs the portable component gates. It
still runs the fixture-backed native WebView gate, React flow, browser policy
suite, and Rust tests; those layers are not presented as proof of the live
install-to-delete boundary.
