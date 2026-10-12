# WinBoat RDP readiness and certificate trust

Every privileged Windows operation (Studio Pro install, launch, uninstall,
authoritative status) runs as a FreeRDP RemoteApp against the WinBoat guest.
This page describes how Mendimaru decides that the RDP endpoint is ready and
which server certificate it trusts (#227).

## Why the host port is not an identity

WinBoat publishes the guest RDP port from a host port range, for example
`127.0.0.1:47300-47309:3389/tcp`. Docker or Podman picks a free port from that
range whenever the container is created. Run Locally forwarding (#146/#153),
its rollback, and some settings changes recreate the container, so the RDP
host port can change after each of them (`47300` → `47301` → `47303` → `47304`
in #227).

FreeRDP stores its trust-on-first-use (TOFU) pins per `host_port`
(`freerdp/server/127.0.0.1_<port>.pem`). Used alone, that store made every
recreation a new first use. An old pin could also linger on a port that a later
recreation hands out again.

## Readiness

Before each RemoteApp attempt, Mendimaru resolves the current published RDP
port. It then sends an X.224 Connection Request with an RDP negotiation request
and waits until two consecutive probes, 500 ms apart, receive an X.224
Connection Confirm. A bare TCP connect is not enough: Docker's userland proxy,
rootless forwarders and QEMU host forwarding accept TCP immediately even when
nothing in the guest listens. The probe never starts TLS, never authenticates
and sends no credentials.

If the endpoint does not become stable within 60 seconds, the operation fails
with `remote_desktop_unavailable` (retryable). It is not reported as a generic
`operation_failed`.

## Certificate trust

Mendimaru keeps one trust anchor per VM identity (container runtime, container
name and RDP host, never the port). The anchor lives in
`$XDG_CONFIG_HOME/mendimaru/freerdp-config/anchors/` (default
`~/.config/mendimaru/freerdp-config/anchors/`), with mode `0700` for the
directory and `0600` for the files.

| State      | FreeRDP policy                           | Per-port FreeRDP pin                                     |
| ---------- | ---------------------------------------- | -------------------------------------------------------- |
| Anchor     | `/cert:fingerprint:sha256:<anchor>,deny` | Rewritten to the anchor before connecting                |
| Legacy pin | `/cert:fingerprint:sha256:<pin>,deny`    | The existing pin of this port, or the one all pins share |
| First use  | `/cert:tofu,deny`                        | Written by FreeRDP when it accepts the certificate       |

- **Promotion.** A certificate becomes the anchor only after Windows has
  returned an HMAC-authenticated operation report through that connection. A
  listener that answers during a recreation therefore can never become
  trusted.
- **Discarding unconfirmed pins.** If a first-use attempt fails, the per-port
  pin that FreeRDP created during that attempt is removed.
- **Pins that are never removed.** Anchors and legacy pins are never removed
  automatically.
- **Migration.** Pins from older versions are reused only when the current
  port has one, or when every pin of the host names the same certificate. If
  they disagree, at least one of them is stale, so the connection starts from a
  confirmed first use instead.
- **Corrupt files.** A truncated per-port pin carries no trust decision, so it
  is cleared rather than blocking every later connection. A corrupt anchor
  fails closed.
- **No prompt.** Every policy includes `deny`. FreeRDP never stops at its
  interactive `Do you trust the above certificate? (Y/T/N)` prompt, which used
  to wait until the operation deadline.

### What counts as a rejected certificate

On a successful first use against WinBoat, FreeRDP 3.15 prints several lines
that look like failures:

- `Certificate verification failure 'self-signed certificate'`
- `WARNING: CERTIFICATE NAME MISMATCH`, because the Windows certificate names
  the guest and not `127.0.0.1`
- a `REMOTE HOST IDENTIFICATION HAS CHANGED … Host key verification failed`
  block for an unknown host, because upstream swaps the new-host and
  changed-host messages

TOFU still accepts the certificate after all of these. Mendimaru treats only
FreeRDP's TLS handshake verdict `certificate not trusted, aborting.` as a
rejection. Such a rejection may be re-probed once within the same operation.
The final result is `remote_desktop_certificate_mismatch` and is never
retryable.

### Resetting trust after reinstalling the Windows VM

A reinstalled guest presents a new certificate, so every operation reports
`remote_desktop_certificate_mismatch`. Resetting trust is an owner decision:

1. Make sure the WinBoat container you expect is running and that no other
   program publishes ports in its RDP range (`docker port WinBoat 3389/tcp`).
2. Remove the anchor and the cached per-port pins:

   ```sh
   rm -rf ~/.config/mendimaru/freerdp-config/anchors \
          ~/.config/mendimaru/freerdp-config/freerdp/server
   ```

3. Run any Windows operation, for example `studio status --refresh`. The first
   authenticated operation records the new anchor.

## Stopping RemoteApp clients

When Mendimaru stops a FreeRDP client, it sends `SIGTERM` first. FreeRDP 3 then
aborts the connection and disconnects the RDP session cleanly. Mendimaru sends
`SIGKILL` only if FreeRDP has not exited after two seconds. A killed client
leaves a half-closed RemoteApp session, and the next connection can inherit it.
