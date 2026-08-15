# WinBoat Studio Pro Run Locally

The `linux-winboat` backend can expose a Mendix application started by Studio
Pro inside the Windows guest to a browser on the Linux host. This is a distinct
Runtime mode from Portable Runtime:

```bash
mendimaru runtime start --mode studio-run-locally --json
mendimaru runtime wait --session-id RUNTIME_SESSION_ID --json
mendimaru runtime url --session-id RUNTIME_SESSION_ID --json
```

The default Windows guest Runtime port is `8080`. Use `--guest-port PORT` only
when the Studio Pro project is explicitly configured to listen on another port.
The option accepts 1024 through 65535. The Linux host port is never supplied by
the caller: Compose asks Docker or Podman for a dynamic port and binds it to
`127.0.0.1` only.

## Start and readiness sequence

`runtime start` performs these steps:

1. Require a healthy WinBoat Guest API and a direct, bounded Compose file.
2. Capture a private `0600` copy of the original Compose file and the current
   `/storage` mount identity.
3. Replace any stale or public mapping for the guest Runtime port with
   `127.0.0.1::<guest-port>/tcp`. Other ports and volumes are unchanged.
4. Recreate WinBoat only when the Compose mapping changed, wait for the guest,
   verify that `/storage` still identifies the same volume, and inspect the
   container for the actual dynamic host port.
5. Probe the resulting `http://127.0.0.1:<host-port>` URL. A TCP mapping alone
   is not readiness. The status remains `starting` or `running` and omits `url`
   until an HTTP response below 500 is received.

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
`url`. A container or guest restart is handled by inspecting the live binding
again; a changed dynamic host port replaces the stored URL only after the new
endpoint passes HTTP readiness.

## Failure and recovery

Compose application is transactional. If recreation, guest startup, binding
inspection, or `/storage` validation fails, Mendimaru restores the captured
Compose bytes, recreates the original WinBoat configuration, waits for the
guest, and rechecks the original storage identity. A failed rollback is
reported as `runtime_compose_recovery_failed` and is never hidden behind the
initial error.

Runtime failures have stable codes:

| Code                              | Meaning                                                                                    |
| --------------------------------- | ------------------------------------------------------------------------------------------ |
| `runtime_guest_offline`           | The Guest API cannot be reached.                                                           |
| `runtime_port_conflict`           | Docker or Podman could not allocate/bind the host port.                                    |
| `runtime_port_forwarding_invalid` | The mapping is absent, duplicated, public, stale, or has no usable host port.              |
| `runtime_not_listening`           | The authenticated Windows probe found no guest TCP listener.                               |
| `runtime_firewall_blocked`        | A guest listener exists but Windows firewall or Mendix port security prevents host access. |
| `runtime_readiness_timeout`       | HTTP readiness expired and a more specific guest diagnosis was unavailable.                |
| `runtime_exited`                  | The explicitly linked Studio Pro process identity ended before readiness.                  |

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
