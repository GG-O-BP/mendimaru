# Issue #227 resolution: Studio start after Run Locally forwarding recreation

## Symptom

After the #225 lock fix, `studio start --version 11.12.6` failed twice with
`{"code":"operation_failed","capability":"studio.start","retryable":false}`.
Each attempt recreated the WinBoat container for Run Locally forwarding. The
RDP host port moved from `47300` → `47301` → `47303` → `47304`, and FreeRDP
logged `Host key for 127.0.0.1 has changed and you have requested strict
checking.`

## Root cause

The certificate never changed. The failure came from how Mendimaru classified
FreeRDP's output on the first connection to a new port.

1. **The certificate was the same.**
   - `e40598f9…` in the report is the `sha256sum` of the pin **file**.
   - The certificate (DER) SHA-256 of all four pins is
     `b9:ac:54:94:…:af:da:87`.
   - That is exactly the fingerprint FreeRDP printed as "sent by the remote
     host".
   - `127.0.0.1_47303.pem` was _created_ at `06:12:17.165Z`, the second of the
     quoted log. That was FreeRDP TOFU recording the pin.
2. **FreeRDP 3.15.0 prints failure-like text on a successful first use.**
   - `tls_print_new_certificate_warn()` runs when no pin exists, but prints the
     "REMOTE HOST IDENTIFICATION HAS CHANGED … Host key verification failed"
     text. The new-host and changed-host messages are swapped upstream.
   - The Windows self-signed certificate also adds `CERTIFICATE NAME MISMATCH`
     and `Certificate verification failure 'self-signed certificate'`.
   - `/cert:tofu` accepts the certificate regardless and continues.
   - Reproduced against the live VM with a scratch trust store: the pin was
     written, and the connection proceeded through licensing and
     authentication.
3. **Mendimaru matched those words.**
   - `RemoteAppProcess::certificate_failed()` treated "certificate" together
     with "mismatch", "verification failure" or "changed" as a rejection.
   - It killed FreeRDP and returned a non-retryable failure without a typed
     code, which surfaced as `operation_failed`.
4. **Pins were per port.**
   - Every recreation moved the port, so the first connection after it was
     always a TOFU, and was always misclassified.
   - A failed start rolls Compose back, which recreates the container again,
     so the next attempt also started on a new port.
   - The one `studio status --refresh` failure after a recreation has the same
     cause.

Two more defects were found on the same path:

- With an existing pin, the fingerprint policy had no `deny`. A real mismatch
  stopped at FreeRDP's interactive prompt until the operation deadline.
- RemoteApp clients were stopped with `SIGKILL`. That leaves a half-closed RDP
  session, which matches the X11 `BadMatch` observed after a forced stop.

## Fix

See [`winboat-rdp-trust.md`](winboat-rdp-trust.md) for the full design.

- Only FreeRDP's TLS handshake verdict `certificate not trusted` counts as a
  certificate rejection.
- One port-independent trust anchor per VM is promoted only after an
  HMAC-authenticated Windows report. The per-port FreeRDP pin is rewritten from
  the anchor before each connection. Unconfirmed TOFU pins from failed attempts
  are removed. Older per-port pins are migrated.
- Every certificate policy carries `deny`, so FreeRDP never waits at a prompt.
- Readiness requires two consecutive X.224 Connection Confirms on the RDP port,
  which is re-resolved for every attempt. A bare TCP connect is not enough.
- New typed causes:
  - `remote_desktop_unavailable` (retryable)
  - `remote_desktop_certificate_mismatch` (not retryable)

  Both appear in the GUI, the CLI, and the operation history. They extend the
  unreleased 6.0.0 contract.

- FreeRDP is stopped with `SIGTERM` first and `SIGKILL` only after a two-second
  grace period.

## Verification

- Unit tests cover:
  - the captured FreeRDP 3.15 first-use output, which is not a rejection, and
    the `deny` rejection output, which is
  - first-use promotion, and survival of the anchor across a port move
  - discarding unconfirmed pins
  - overwriting stale per-port pins
  - legacy migration when pins agree and when they disagree
  - corrupt pins and anchors
  - private file modes
  - X.224 readiness against proxy, garbage, silent, refused and flapping
    endpoints
  - typed, non-retryable certificate failures
  - graceful and forced termination
- Live FreeRDP 3.15.0 checks against the #227 VM: `/cert:tofu,deny` accepts and
  records a first use, and rejects a differing stored pin immediately.
  `/cert:fingerprint:sha256:<anchor>,deny` connects even when a stale
  per-port pin differs.
