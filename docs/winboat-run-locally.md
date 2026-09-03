# WinBoat Studio Pro Run Locally

The `linux-winboat` backend can expose a Mendix application started by Studio
Pro inside the Windows guest to a browser on the Linux host. This is a distinct
Runtime mode from Portable Runtime:

```bash
mendimaru runtime start --mode studio-run-locally --json
mendimaru runtime wait --session-id RUNTIME_SESSION_ID --json
mendimaru runtime url --session-id RUNTIME_SESSION_ID --json
```

The default Studio Pro Runtime port is `8080`. There is no manual Runtime-port
CLI option. When `--studio-session-id` is supplied, Mendimaru reads
`MXCONSOLE_RUNTIME_PORT` from that Studio project's bounded `.launch` settings.
Without a session, it uses the workspace `.launch` ports when every discovered
project agrees, and falls back to `8080` when no project publishes a port.
Conflicting workspace ports are rejected instead of guessing. Compose always
uses the same loopback address and port: `127.0.0.1:<port>:<port>/tcp`, and the
browser-facing URL is always `http://localhost:<port>/`.

## Start and readiness sequence

`runtime start` performs these steps:

1. Require a healthy WinBoat Guest API and a direct, bounded Compose file.
2. Capture a private `0600` copy of the original Compose file and the current
   `/storage` mount identity.
3. Replace any stale, dynamic, or public mapping for the Studio Runtime port
   with `127.0.0.1:<port>:<port>/tcp`. Other ports and volumes are unchanged.
4. Recreate WinBoat only when the Compose mapping changed, wait for the guest,
   verify that `/storage` still identifies the same volume, and inspect the
   container for the expected fixed host port.
5. Probe the resulting `http://127.0.0.1:<port>` endpoint. A TCP mapping alone
   is not readiness. The status remains `starting` or `running` and omits `url`
   until an HTTP response below 500 is received.

The published URL uses `localhost` so it matches the address Studio Pro users
expect. Backend readiness probes pin `127.0.0.1` explicitly because Compose
publishes only the IPv4 loopback address; Chromium and other browsers fall back
from `::1` to `127.0.0.1` automatically, and this behavior is covered by the
browser E2E suite.

This split accommodates the interactive Studio Pro **Run Locally** action.
Prepare the Runtime session first when Compose does not yet contain the
mapping, then start Run Locally in the guest and call `runtime wait`. If the
mapping was already prepared, `--studio-session-id` may bind the Runtime to an
exact `studio-<pid>-<start-ticks>` identity. Without that option, Mendimaru can
attach the only observed Studio session; `studioState` remains `unknown` when
the identity is ambiguous.

`RuntimeStatus` reports `backend=linux-winboat`,
`mode=studio-run-locally`, distinct `hostPort` and `guestPort`, `studioState`,
and `httpReady`. `state=ready` always implies `httpReady=true` and a loopback
`url` (`http://localhost:<port>`). A container or guest restart is handled by
inspecting the live binding again; the URL changes only after the new endpoint
passes HTTP readiness.

While `runtime status` and `runtime wait` poll for readiness, they use only the
forwarded application HTTP endpoint. They do not repeatedly open RemoteApp or
query Windows sessions; an authenticated Windows diagnosis runs once only after
the bounded wait expires. Future readiness watchers must preserve this
isolation so monitoring cannot disturb the interactive Studio session.

## Failure and recovery

Compose application is transactional. If recreation, guest startup, binding
inspection, or `/storage` validation fails, Mendimaru restores the captured
Compose bytes, recreates the original WinBoat configuration, waits for the
guest, and rechecks the original storage identity. A failed rollback is
reported as `runtime_compose_recovery_failed` and is never hidden behind the
initial error.

Runtime failures have stable codes:

| Code                              | Meaning                                                                                                                        |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `runtime_guest_offline`           | The Guest API cannot be reached.                                                                                               |
| `runtime_port_conflict`           | The port is owned by another active Mendimaru Runtime session, or Docker/Podman could not allocate it for an external program. |
| `runtime_port_forwarding_invalid` | The mapping is absent, duplicated, public, stale, or has no usable host port.                                                  |
| `runtime_not_listening`           | The authenticated Windows probe found no guest TCP listener.                                                                   |
| `runtime_firewall_blocked`        | A guest listener exists but Windows firewall or Mendix port security prevents host access.                                     |
| `runtime_readiness_timeout`       | HTTP readiness expired and a more specific guest diagnosis was unavailable.                                                    |
| `runtime_exited`                  | The explicitly linked Studio Pro process identity ended before readiness.                                                      |

The Windows diagnosis runs as the existing hash-pinned, authenticated
PowerShell operation. It inspects the exact numeric guest port and returns only
an allowlisted diagnostic token; firewall rules, paths, credentials, and raw
PowerShell output do not enter the common contract or logs.

## Stop and exposure boundary

`runtime stop` restores the exact original Compose file and recreates WinBoat,
which terminates the guest Runtime and any active Studio process. It verifies
the `/storage` identity again before recording `stopped`. This disruptive
boundary is intentional because Studio Pro does not expose a safe unattended
Run Locally stop API yet. The managed Compose digest must still match; a
concurrent user edit is preserved and stop returns
`runtime_compose_recovery_failed` instead of overwriting it.

The host URL is always IPv4 loopback. `0.0.0.0`, a LAN address, multiple
bindings, host networking, a missing `/storage` volume, and a caller-selected
host port are rejected. Native Windows and macOS adapters do not read or change
Compose; their future `studio-run-locally` implementations own native port and
process handling.
