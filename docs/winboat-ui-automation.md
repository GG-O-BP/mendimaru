# Studio UI automation on Linux + WinBoat (#21)

The provider uses the existing Studio session keeper and authenticated guest
channel. A persistent MTA Windows worker runs beside the selected Studio process.
The CLI never starts a new RemoteApp connection for a UI command.

**Windows native is deferred.** Its UI capabilities remain unsupported. The
common request API and `UiAutomationBackend` boundary allow a future local
transport without changing callers. WinBoat evidence is not native Windows
Studio evidence.

## Supported surface

The initial adapters are pinned to Studio **10.24.26.0** and **11.12.4.0**, the
file versions investigated in [#16](winboat-studio-uia-spike.md). Other versions
fail with `unsupported_capability` / `ui-unsupported-version`. This implements
the bounded helper decision; it does not enable arbitrary canvas/widget editing.

Start Studio with this version of the CLI, then use its `studioSessionId`:

```sh
mendimaru studio start --version 11.12.4 --json
mendimaru ui capabilities --session-id studio_PID_TICKS --json
mendimaru ui tree --session-id studio_PID_TICKS --json
mendimaru ui find --session-id studio_PID_TICKS --role MenuItem --name File --json
mendimaru ui wait --session-id studio_PID_TICKS --role Button --name Stop --timeout-ms 30000 --json
mendimaru ui screenshot --session-id studio_PID_TICKS --json
mendimaru ui release --session-id studio_PID_TICKS --json
```

`studio_PID_TICKS` above is a placeholder: use the exact `studio-<PID>-<UTC ticks>`
returned by Studio start. Names match exactly and are case sensitive. Roles are
UIA control types without the `ControlType.` prefix. `--automation-id` combines
with role/name; `--scope-id` confines a lookup to a previously observed element.
An empty selector, guessed process ID, script, or arbitrary executable is invalid.

Every observation starts a new element generation. Use IDs from the latest
observation; previous IDs fail closed. Scopes are resolved before a new lookup.
Find returns all unique matches; wait requires exactly one enabled, visible
match. Duplicate provider entries are deduplicated by complete UIA runtime ID.
Multiple distinct matches fail wait with `ui-ambiguous-element`. A truncated
scan cannot prove absence or uniqueness and fails find/wait explicitly.

| Action           | Preconditions and result                                                                                                                                            |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `invoke`         | Requires the element's InvokePattern. A completed call confirms semantic dispatch; wait separately for the app effect.                                              |
| `click`          | Uses SelectionItemPattern or TogglePattern and verifies the resulting state. Coordinate/canvas clicking is unsupported.                                             |
| `focus`          | Requires a visible enabled element in the foreground owned window; verifies keyboard focus.                                                                         |
| `set-value`      | Only the Properties Name editor, writable ValuePattern, and a Mendix identifier of at most 100 ASCII characters. Verifies editor readback.                          |
| `keyboard-input` | Only `Tab`, `F5`, `Ctrl+G`, `Ctrl+S`, `Enter`, `Right`, `Escape`, on a native WPF/WinForms/Win32 focus target. F5 additionally requires observed project readiness. |

```sh
mendimaru ui action --session-id SESSION --element-id ELEMENT --action focus --json
printf '%s' 'renamedWidget' | mendimaru ui action --session-id SESSION \
  --element-id NAME_EDITOR --action set-value --value-stdin --json
mendimaru ui action --session-id SESSION --element-id NATIVE_ELEMENT \
  --action keyboard-input --key F5 --json
mendimaru ui wait --session-id SESSION --condition running --timeout-ms 60000 --json
```

Values enter through stdin, never `--value` or an arbitrary PowerShell expression.
Password elements are excluded from lookup/input and their names/values are
redacted in trees. Editor readback does not mean the model has been saved: commit
focus changes and save explicitly, then verify the intended application result.
A delivered shortcut or selected preview outline alone is not success evidence.

Semantic waits are `project-ready`, `building`, `deploying`, `starting-runtime`,
`running`, and `modal`. The tree
reports `unknown` when no supported observation proves a state. Readiness uses
native shallow WPF status/Run controls and the loaded app explorer root; runtime state uses the enabled Console
Stop control. Busy phases retain their observed status text; a phase not observed
is never inferred as completed. These are Studio UI observations, **not HTTP or browser health**.
Use Runtime/browser verification for those boundaries. Dialogs retain their IDs,
names, and conservative login/conversion/update/unknown classification. The
provider never dismisses a dialog automatically or sends input through a modal.

## Capture and diagnostics

Screenshot defaults to the selected Studio main window. `--window-id ELEMENT`
selects a window from the tree inventory; `--region x,y,width,height` clips
physical window-relative pixels. Regions must fit the selected window. No
screen coordinates are accepted as input actions.

The returned `ArtifactDescriptor` contains a private local PNG location, size,
and SHA-256. The method is PrintWindow; inspect its pixels before claiming usable
rendering. Linux RemoteApp pixels can differ from the Windows capture. Artifacts
are private, independently owned directories in the CLI cache and remain until
the caller removes the specific returned directory. Screenshot retention does
not delete another run's artifacts.

`ui tree` includes selected process/start identity, exact adapter file version,
interactive session/helper IDs, main/owned window handles, focus, minimized and
DPI state, parent-linked nodes, dialogs, limits, and explicit truncation. The
worker never enumerates the desktop outside that Studio's visible windows.
The existing `studio status` command remains the process/session inventory.

## Ownership, security, and limits

- Linux IPC uses the keeper's private Unix socket, ownership/mode checks, and
  same-UID peer credentials. Guest requests/replies use the existing per-launch
  HMAC channel, monotonic request sequences, unpredictable request IDs, and
  signed deadlines. A signed stale reply cannot satisfy a new request.
- A request cannot choose worker code, executable paths, guest output paths, or
  other process targets. The authenticated launch script embeds the worker;
  its guest temporary directory has a current-user-only ACL. Credentials and
  HMAC keys stay out of host and guest argv, public reports, and diagnostic logs.
  A private, one-use bootstrap is delivered through an RDP redirected directory,
  hash-pinned in the guest command, removed before execution and cleaned on child
  exit. The bootstrap directory exposes no other host files.
- Each request verifies PID, start ticks, executable path, exact file version,
  same nonzero interactive session, active RDP state, and Default input desktop.
  Lock/disconnect fails closed. Focus and keyboard input require the selected RemoteApp to have foreground
  focus; a denied foreground transition returns `ui-foreground-lost`. Explicit `ui reconnect --session-id SESSION` revalidates identity and supports the command timeout and cancellation; a new worker
  generation invalidates old elements. A failed reconnect retains the original
  session observer and project lease. A verified replacement sends a signed
  retirement request to the previous monitor, preserving Studio. Studio handoff after binding never
  silently retargets input to another PID.
- The worker is a killable MTA child in a Windows job with kill-on-parent-close
  and a 512 MiB process-memory ceiling. A hung UIA call cannot hang the parent
  Studio monitor. Release, crash, timeout and cancellation discard the worker;
  the next request lazily starts a fresh one. Studio and runtime are preserved.
- The keeper serializes requests and holds VM use through the operation and
  cancellation response. Conservative exclusive VM use also excludes other
  cooperating UI/lifecycle requests. External controllers and older versions
  do not participate in this advisory lock.
- Defaults: 15 s command budget; explicit 100–60,000 ms; 3,000 nodes, depth 48,
  16 windows, 256-character text fields, 8 MiB PNG, 16 MiB signed reply. Capture
  is limited to 4096×4096 and 8,388,608 pixels. UIA collection may return a
  partial tree; querying a partial tree as complete is forbidden.
- UI effects already dispatched before cancellation cannot be undone. Callers
  must inspect current state before retrying a mutation. Old keepers without UI
  support require an explicit Studio close/start; the CLI does not replace an
  active RDP connection as a hidden compatibility fallback.

## Errors and versioning

The outer contract is **5.0.0** because the closed CLI command enum and supported
UI result semantics changed. Persisted v4 Runtime/build/browser records remain
readable without rewriting original snapshots or artifact descriptors. Older,
incompatible Runtime records retain the existing quarantine/forget behavior.

Errors use existing backend codes and exact path-free messages:

- `unsupported_capability`: `ui-unsupported-version`, `ui-unsupported-element`.
- `external_process_timeout`: `ui-helper-timeout`.
- `external_process_cancelled`: `ui-cancelled`.
- `precondition_failed`: `ui-session-unavailable`, `ui-helper-exited`,
  `ui-no-interactive-desktop`, `ui-wrong-session`, `ui-stale-element`,
  `ui-ambiguous-element`, `ui-tree-truncated`, `ui-modal-blocked`,
  `ui-foreground-lost`, `ui-effect-unverified`, `ui-capture-failed`,
  `ui-request-expired`, `ui-bridge-untrusted`, `ui-provider-failed`.

Known CLR exception types and signed HRESULTs are returned as bounded
`uia:TYPE:CODE` diagnostic references. Exception messages, source, stack traces
and arbitrary type names never pass the public error boundary.

Unsupported native adapters retain the same backend error envelope and never
request WinBoat credentials, RDP, or shared files.

The MTA/process isolation follows Microsoft's
[UI Automation threading guidance](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading).
