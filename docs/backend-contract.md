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
UI automation and browser execution remain explicitly unsupported. `mac-native`
is registered but reports all operations as unsupported until issue #11
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

`ArtifactDescriptor` links every artifact to a session and backend. Local
location, media type, digest, size, and backend diagnostic reference are
optional. This keeps platform paths out of the portable required shape while
allowing an adapter to provide a verified local artifact.

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

The current schema version is `2.0.0` and follows semantic versioning. Version
2 adds multi-mode capability discovery, explicit Runtime backend/readiness,
WinBoat Run Locally status fields, and its distinct failure codes:

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

`npm run check` runs lint, format, frontend tests, Rust contract/fake-adapter
tests, real CLI process tests, and JSON Schema validation. Linux maintainers can
run the destructive full adapter lifecycle only against a disposable WinBoat
test version with an official installer already in the shared cache:

```bash
MENDIMARU_E2E_ALLOW_MUTATION=1 \
MENDIMARU_E2E_VERSION=11.13.0 \
cargo test --manifest-path src-tauri/Cargo.toml \
  platform::tests::live_e2e_linux_winboat_backend_lifecycle \
  -- --ignored --exact --nocapture --test-threads=1
```

The test normalizes that exact version to absent, installs it through the common
adapter, verifies all progress phases and exact-version detection, launches a
real Studio window, uninstalls it officially, and verifies it is absent again.
The non-destructive live session suite can run against an existing WinBoat VM:

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  winboat::sessions::tests::live_e2e_lists_sessions_and_rejects_an_ended_identity \
  -- --ignored --exact --nocapture --test-threads=1
```

It queries current-user sessions through the authenticated RemoteApp transport
and verifies that reconnect and close both reject a stale PID/start-time pair.
