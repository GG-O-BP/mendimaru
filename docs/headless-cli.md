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
mendimaru studio status [--session-id STUDIO_SESSION_ID]
mendimaru studio stop --session-id STUDIO_SESSION_ID
mendimaru project list
mendimaru project version --project-id PROJECT_ID
mendimaru runtime build --project-id PROJECT_ID [--clean]
mendimaru runtime start --project-id PROJECT_ID [--clean]
mendimaru runtime status --session-id RUNTIME_SESSION_ID
mendimaru runtime wait --session-id RUNTIME_SESSION_ID
mendimaru runtime url --session-id RUNTIME_SESSION_ID
mendimaru runtime stop --session-id RUNTIME_SESSION_ID
mendimaru runtime logs --session-id RUNTIME_SESSION_ID [--cursor CURSOR]
mendimaru operation list
mendimaru operation status --operation-id OPERATION_ID
mendimaru operation retry --operation-id OPERATION_ID
```

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
- `--json` and `--ndjson` are mutually exclusive.

The CLI accepts version strings and opaque IDs only. It has no project-path,
installer-URL, credential, password, or token option. It never opens a file
dialog or confirmation window.

## Result and error streams

A successful command writes exactly one JSON result to stdout (after any
NDJSON progress events) and leaves stderr empty. A failed command leaves stdout
empty and writes exactly one JSON error to stderr. The result envelope includes
the schema version, normalized command name, host platform, selected backend,
invocation session ID, and immutable capability snapshot. Mutation successes
also include their persistent operation ID; Studio session commands include the
Studio session ID when one was selected.

Runtime lifecycle results include an independent `runtimeSessionId`. Runtime
status, build artifacts, and bounded log batches validate against
[`runtime.schema.json`](../schemas/runtime.schema.json); see
[`portable-runtime.md`](portable-runtime.md) for exact-version, readiness,
cache, licensing, and secret-handling rules.

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

`runtime build` and `runtime start` always derive their exact version from the
opaque project ID. `--clean` rebuilds only that project's cached portable
package. Runtime configuration and credentials have no CLI flags; callers may
provide the bounded `MENDIMARU_RUNTIME_ENV_JSON` process environment described
in the Portable Runtime guide. A Runtime URL is withheld until HTTP readiness.

On Linux, a detached per-session keeper owns the verified FreeRDP process after
`studio start` returns. `studio status` and `studio stop` communicate with that
keeper through a user-owned `0700` cache directory and `0600` Unix socket. This
avoids opening a second RDP connection, which would replace the active
RemoteApp connection. Socket names are irreversible hashes of Studio session
IDs; no password, project path, or command line is persisted. A stale or
untrusted socket is never treated as a live session.

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
