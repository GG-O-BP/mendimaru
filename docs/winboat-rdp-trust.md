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
port. A missing mapping is a retryable readiness failure, not permission to use
the potentially stale configured range start. It then sends an X.224 Connection Request with an RDP negotiation request
and waits until two consecutive probes, 500 ms apart, receive an X.224
Connection Confirm. A bare TCP connect is not enough: Docker's userland proxy,
rootless forwarders and QEMU host forwarding accept TCP immediately even when
nothing in the guest listens. The probe never starts TLS, never authenticates
and sends no credentials.

If the endpoint does not become stable within 30 seconds, the operation fails
with `remote_desktop_unavailable` (retryable). It is not reported as a generic
`operation_failed`.
Cancellation also interrupts this wait; it does not wait out the readiness
deadline or start a new RemoteApp after cancellation is observed.

## Certificate trust

Mendimaru keeps one trust anchor per VM identity (container runtime, container
name and RDP host, never the port). The anchor lives in
`$XDG_CONFIG_HOME/mendimaru/freerdp-config/anchors/` (default
`~/.config/mendimaru/freerdp-config/anchors/`), with mode `0700` for the
directory and `0600` for the files.

FreeRDP runs with a private, attempt-specific `XDG_CONFIG_HOME` beneath the
application trust directory. It never writes its TOFU output directly into the
persistent legacy pin namespace.

| State      | FreeRDP policy                           | Pin in the private attempt directory             |
| ---------- | ---------------------------------------- | ------------------------------------------------ |
| Anchor     | `/cert:fingerprint:sha256:<anchor>,deny` | Seeded from the immutable VM anchor              |
| Legacy pin | `/cert:fingerprint:sha256:<pin>,deny`    | Seeded from existing, unambiguous trust          |
| First use  | `/cert:tofu,deny`                        | Written by FreeRDP, not yet persisted as trusted |

- **Promotion.** A certificate becomes the anchor after the first valid
  HMAC-authenticated Windows operation report, including an application
  failure report. Invalid MACs and malformed reports never promote a pin.
  Anchor writes are create-only: concurrent confirmations cannot replace
  existing trust. Persistence errors are returned without retrying an
  operation that might already have taken effect.
- **Discarding unconfirmed pins.** Failed, cancelled and abandoned attempts
  lose their private pin when their owner is dropped. Even an abrupt process
  exit cannot turn a leftover attempt directory into legacy trust on retry:
  migration scans only the persistent `freerdp/server/` namespace.
- **First-use limitation.** TOFU is not independent server authentication.
  The report MAC protects the operation channel, but its key was delivered
  over this RDP connection. Existing or ambiguous trust must therefore never
  silently revert to TOFU.
- **Pins that are never removed.** Anchors and legacy pins are never removed
  automatically.
- **Migration.** Pins from older versions are reused only when the current
  port has one, or when every pin of the host names the same certificate. If
  the new port has no pin and older pins disagree, migration fails with
  `remote_desktop_certificate_mismatch` rather than accepting a new certificate.
- **Corrupt files.** Invalid prior trust fails closed and is preserved for
  owner review. A valid anchor can repair a corrupt per-port cache after
  confirmation, but no anchor means no automatic trust reset. Pins must be
  bounded regular files; symlinks are refused at open time. Trust subdirectory
  symlinks are also refused rather than followed or chmodded.
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
