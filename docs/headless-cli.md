# Headless CLI contract

Mendimaru exposes the same application orchestration used by the desktop UI as
a GUI-free command line interface. Linux packages install the executable as
`/usr/bin/mendimaru`; Windows installers include the same `mendimaru.exe`
binary as the desktop application.

## Commands

```text
mendimaru capabilities [--backend BACKEND]
mendimaru env status
mendimaru env ensure
mendimaru studio list
mendimaru studio install --version VERSION [--force-redownload]
mendimaru studio uninstall --version VERSION
mendimaru studio start --version VERSION [--project-id PROJECT_ID]
mendimaru studio status [--session-id STUDIO_SESSION_ID] [--refresh | --orphans]
mendimaru studio stop --session-id STUDIO_SESSION_ID
mendimaru project list
mendimaru project version --project-id PROJECT_ID
mendimaru runtime build --project-id PROJECT_ID [--clean]
mendimaru runtime start --project-id PROJECT_ID [--clean] [--mode portable]
mendimaru runtime start --mode studio-run-locally [--studio-session-id STUDIO_SESSION_ID]
mendimaru runtime list
mendimaru runtime status --session-id RUNTIME_SESSION_ID
mendimaru runtime wait --session-id RUNTIME_SESSION_ID
mendimaru runtime url --session-id RUNTIME_SESSION_ID
mendimaru runtime stop --session-id RUNTIME_SESSION_ID
mendimaru runtime forget --session-id RUNTIME_SESSION_ID
mendimaru runtime logs --session-id RUNTIME_SESSION_ID [--cursor CURSOR]
mendimaru browser doctor
mendimaru browser install chromium
mendimaru browser test (--base-url URL | --runtime-session-id RUNTIME_SESSION_ID) --suite-path SUITE_JSON
mendimaru browser artifacts --session-id BROWSER_SESSION_ID
mendimaru operation list
mendimaru operation status --operation-id OPERATION_ID
mendimaru operation retry --operation-id OPERATION_ID
```

`mendimaru --help` (or `mendimaru -h`) prints this command summary immediately,
and `COMMAND --help` prints command-specific usage. Both exit `0`. Help is
handled before localization, configuration, capability snapshot creation, or
backend access, so it also works while WinBoat and the guest are offline.

An unrecognized command never falls back to the desktop application. It fails
immediately with exit code `2`, one machine-readable error envelope, a null
capability snapshot, and a safe command hint. For example, `mendimaru status`
points the caller to `env status` or `studio status`.

All commands default to one JSON result. `--json` selects that format
explicitly. `--ndjson` emits zero or more structured progress events followed
by one result for long-running install and retry commands. Global flags may be
placed anywhere in the command:

- `--backend linux-winboat|windows-native|mac-native` requires an exact match
  with the host. A mismatch fails before configuration, filesystem, or backend
  access; it never selects a fallback.
- `--timeout-seconds N` accepts 1 through 3600 seconds and defaults to 300.
  Reaching the timeout cancels the in-process future. `Ctrl+C` uses the same
  cancellation boundary.
- `--snapshot` opts in to the immutable capability snapshot in that response.
  It is omitted by default; `capabilities` still returns the full snapshot as
  its `data` payload.
- `--json` and `--ndjson` are mutually exclusive.

The CLI accepts version strings and opaque IDs only. It has no project-path,
installer-URL, credential, password, or token option. It never opens a file
dialog or confirmation window.

## Result and error streams

A successful command writes exactly one JSON result to stdout (after any
NDJSON progress events) and leaves stderr empty. An invocation or backend
failure leaves stdout empty and writes exactly one JSON error to stderr. An
executed `browser test` with failed assertions or diagnostic policy violations
is the deliberate exception: it writes its complete result to stdout and exits
`1`, so failure artifacts remain agent-readable. The result envelope includes
the schema version, normalized command name, host platform, selected backend,
invocation session ID, and command data. The immutable capability snapshot is
not duplicated into ordinary responses; pass `--snapshot` when a caller needs
it. Mutation successes also include their persistent operation ID; Studio
session commands include the Studio session ID when one was selected.

Runtime lifecycle results include an independent `runtimeSessionId`. Runtime
session lists, forget results, status, build artifacts, and bounded log batches
validate against
[`runtime.schema.json`](../schemas/runtime.schema.json); see
[`portable-runtime.md`](portable-runtime.md) for exact-version, readiness,
cache, licensing, and secret-handling rules, and
[`winboat-run-locally.md`](winboat-run-locally.md) for Linux host/Windows guest
port forwarding and recovery. Browser installation, suites, policy controls,
artifacts, and secret handling are documented in
[`browser-testing.md`](browser-testing.md).

On Linux WinBoat, `runtime list` discovers current and preserved Studio Run
Locally cache records without returning host paths or Compose locations. Each
summary contains only the opaque ID, backend, mode, state, safe timestamps and
ports when present, Studio link, incompatibility reason, and whether the record
is eligible for explicit invalidation. `runtime forget` is deliberately not a
stop replacement: current `starting`, `running`, or `ready` records are rejected
with `precondition_failed`. A stopped/failed or incompatible record is preserved
as an auditable invalidation and removed from future ID lookup and port reuse.
Windows native currently reports this cache-management surface as an
unsupported capability until its native Runtime record lifecycle has the same
contract.

Exit codes are stable:

| Code | Meaning                                                       |
| ---: | ------------------------------------------------------------- |
|  `0` | Command completed                                             |
|  `1` | Operation or precondition failed, timed out, or was cancelled |
|  `2` | Invalid command or argument                                   |
|  `3` | Backend mismatch or unsupported capability                    |

Responses validate against
[`cli-response.schema.json`](../schemas/cli-response.schema.json). NDJSON
progress events validate against
[`cli-event.schema.json`](../schemas/cli-event.schema.json). Backend errors use
[`backend-error.schema.json`](../schemas/backend-error.schema.json).
Sanitization replaces free-form backend messages with code-specific safe text,
but it preserves the structured cause code and retry policy. For example, a
WinBoat launch that cannot link a Runtime record reports
`runtime_session_not_found`, not a generic `operation_failed`; timeout,
cancellation, and external-process interruption codes also remain distinct.
Exit code `1` still covers all of these operational failures.

## Polling and lightweight status

Use the default `studio status` summary for polling. On Linux it returns the
trusted keeper-owned session set without opening RDP or enumerating the Windows
guest. Use `studio status --refresh` when an authoritative guest-wide query is
required; that operation can take longer because it verifies exact installed
versions and process identities. For a selected session, `studio status
--session-id ID` checks the keeper first and falls back to an authoritative
lookup only when that keeper record is unavailable.

`studio status --orphans` performs the authoritative query and returns only
Studio Pro sessions not owned by a Mendimaru keeper. Each returned opaque ID can
be inspected and explicitly passed to `studio stop`; exact Windows PID/start
tick verification still protects unrelated user-started processes.

`studio stop` is idempotent when Mendimaru can authoritatively observe that the
requested session is already gone. If a keeper stop report races with final
cleanup, the CLI verifies the authoritative session list once; an absent target
returns `ok: true` and exit `0`, while a still-running target or an unverifiable
status query preserves the original failure.

The same confirmed-dead transition removes a Mendix `.mpr.lock` only when it is
a direct, bounded, exactly shaped JSON file whose `ProcessId` matches the exact
Studio session that was just observed absent. Locks for other processes and
malformed, linked, oversized, or ambiguous files are deliberately retained.

If Windows reports that Studio Pro started but a later host-side registration
or Runtime-link step fails, Linux cleanup performs the same conservative
transition. Mendimaru records the RemoteApp, Studio stop, Runtime stop, and
authoritative absence result in a private `<operation-id>.cleanup.json` report
without host paths. A `.mpr.lock` is removed only when that authoritative guest
query succeeds and the exact Studio identity is absent; a live or unverifiable
session retains the lock. The failed operation is retryable only when cleanup
recorded successful Runtime recovery and confirmed absence. A Runtime stop or
status-query failure remains non-retryable so a caller does not repeatedly
enter an ambiguous state.

Poll at an interval larger than the command duration, retain the previous
terminal error, and stop after a bounded number of attempts. Keep `--snapshot`
out of polling requests; retrieve it separately with `capabilities` when a
workflow actually needs backend feature negotiation.

## Projects and exact versions

`project list` returns a `project_<sha256>` identity, display name, required
Studio Pro version, and last-modified time. It never returns the host path, UNC
path, workspace, or project contents. The ID is resolved against a fresh scan
inside the configured workspace on every command.

When `studio start` includes `--project-id`, the requested version must exactly
match the one unambiguous version declared by that project. A missing,
ambiguous, or mismatched version fails before an operation record is created or
the platform backend is contacted. There is no nearest-version fallback,
implicit upgrade, or mutation of the `.mpr` file.

Portable `runtime build` and `runtime start` derive their exact version from
the opaque project ID. `--clean` rebuilds only that project's cached portable
package. Runtime configuration and credentials have no CLI flags; callers may
provide the bounded `MENDIMARU_RUNTIME_ENV_JSON` process environment described
in the Portable Runtime guide.

On Linux, `--mode studio-run-locally` selects the WinBoat adapter and does not
build a package or accept a project ID. It records the Windows guest port
(default `8080`) and mirrors it to the same Linux loopback port. The optional
Studio session ID must be an opaque process/start-time identity identifying a
live Studio session. It is validated before a Runtime session ID, session
record, Compose backup, or port-forwarding artifact is created. A Runtime URL
is withheld in both modes until HTTP readiness. WinBoat-specific fields remain
optional in the common schema and native adapters never inspect Compose.

Immediately after WinBoat recreation, Guest API health can precede RemoteApp
readiness. Mendimaru therefore waits for a bounded RemoteApp endpoint window and
classifies an endpoint or transient RemoteApp startup interruption as
`external_process_interrupted` with `retryable: true`. A Studio launch timeout is
also retryable; persistent Windows operation failures retain their authenticated
exit-code diagnostics.

On Linux, a detached per-session keeper owns the verified FreeRDP process after
`studio start` returns. `studio status` and `studio stop` communicate with that
keeper through a user-owned `0700` cache directory and `0600` Unix socket. This
avoids opening a second RDP connection, which would replace the active
RemoteApp connection. Socket names are irreversible hashes of Studio session
IDs; no password, project path, or command line is persisted. A stale or
untrusted socket is never treated as a live session. Stop uses an authenticated,
replay-protected request for the exact PID and process start tick through the
retained connection and waits for Windows to report that process gone; killing
only the local FreeRDP client never counts as a successful stop.

External host project selection remains GUI-only. The headless CLI continues to
resolve opaque project IDs from a fresh configured-workspace scan and accepts no
raw host path option. An external-project Studio session whose temporary drive
has ended is reported as non-reconnectable; callers must use the GUI to select
the `.mpr` again rather than attempting to recover a host path from operation
history.

## Interruption and retry

Install, uninstall, and launch use the same atomic host-only operation history
as the GUI. A process interrupted by timeout, `Ctrl+C`, termination, or host
restart is reported as `interrupted` when `operation list` or `operation status`
reconciles the history. Safe terminal records can be resumed with `operation
retry`; project launches remain non-retryable until the caller supplies the
opaque project ID and exact version again.

Passwords, tokens, installer URLs, project paths, Windows command lines, and
diagnostic observations are excluded from CLI DTOs, operation records, and
checked-in fixtures. Backend diagnostic text is reduced to stable error codes
and allowlisted messages before serialization.
