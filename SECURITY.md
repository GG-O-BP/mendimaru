# Security policy and WinBoat trust boundary

## Supported security model

Mendimaru treats the Linux host, its user account, and the configured WinBoat Windows administrator account as trusted. The shared workspace is an untrusted transport for privileged operations: another process that can write there may delete files or cause denial of service, but it must not be able to replace an executable or forge a successful operation.

Before Windows starts an installer, Studio Pro, or an official uninstaller, Mendimaru requires all of the following:

- the canonical target remains below its configured install, data, shared, or staging root;
- no target or ancestor below that root is a junction, symbolic link, or other reparse point;
- the file has a valid Authenticode status and an exact `CN` or `O` component for Mendix Technology B.V. or Siemens AG;
- the SHA-256 value is stable before and after signature verification;
- installers on Linux match both the host-validated cache digest and a fresh, unique guest staging copy.

An installer is never reused merely because its length matches. Uninstall refuses to recurse over a version directory when trusted uninstall metadata is missing.

## Command and result channel

Linux writes operation scripts into its application-owned directory under the shared workspace using unpredictable names and create-new semantics. The expected script SHA-256 is held in the host process. Windows checks that digest, copies the script to a unique `%ProgramData%\Mendimaru\Commands` file, checks the source and copy again, executes only the private copy under `RemoteSigned`, and removes it afterward.

Each attempt has a CSPRNG request ID, nonce, and 256-bit HMAC key. The key is sent inside the TLS-protected RDP operation and is never written to the shared workspace. Result files use an atomic, versioned envelope containing the request identity, monotonic sequence, base64 payload, and HMAC-SHA256. The host rejects malformed, oversized, stale, replayed, symlinked, or unauthenticated results.

While a Studio RemoteApp is retained, close requests use the same per-attempt identity and key through a create-new control envelope. Windows accepts only a positive, increasing sequence and a closed payload matching the exact session ID, PID, and process start tick before calling `CloseMainWindow`. Invalid, replayed, overwritten, or retargeted requests never close a process. The host waits for a later authenticated empty-session report; terminating only the local FreeRDP client is not reported as a successful Studio stop.

## Persistent operation history

The operation center persists only an allowlisted host-side summary in the application's host-only configuration directory, outside the untrusted shared workspace: random operation ID, operation kind, target version, protected-project marker, state, stage, bounded progress, timestamps, stable error code, optional Windows exit code, and retryability. It does not store project paths, command payloads, download URLs, credentials, HMAC keys, or raw backend errors. The history file is bounded, schema-checked, rejected if it or an application-owned ancestor is a symbolic link, written with private permissions on Unix, and replaced through a synced temporary file.

An HMAC report cannot be re-authenticated after its process-local key is gone. On restart, Mendimaru therefore changes stale running host records to interrupted. A one-time migration may correlate an older regular, bounded Windows report by its constrained filename, but it never reads that report payload or treats it as evidence of success. Clearing history removes terminal summary records only and deliberately preserves operation reports, scripts, installers, and active records.

## RDP and network exposure

FreeRDP receives the Windows password through standard input rather than argv. Mendimaru uses an application-scoped FreeRDP TOFU certificate store under the user's configuration directory. The first connection pins the WinBoat RDP certificate; a later certificate mismatch fails before a privileged operation starts. Protect the first connection and the Linux user configuration directory. If the VM is intentionally rebuilt and its certificate changes, inspect the cause before removing the Mendimaru-specific pin.

Privileged operations are refused unless both the configured Guest API/RDP endpoints and the container runtime's `7148/tcp` and `3389/tcp` bindings are explicitly loopback-only. Do not publish these ports to a LAN or the Internet.

## Container and VM residual risk

The official WinBoat/dockur Windows configuration may require a privileged container, `CAP_NET_ADMIN`, KVM and device access, disabled container security labels, and read-write mounts for VM storage and the shared workspace. Mendimaru validates exposure but does not silently rewrite those runtime requirements because doing so can prevent the VM from booting.

The Windows guest remains a security boundary, not a guarantee against a hypervisor, kernel, container-runtime, or device-emulation escape. A successful escape from this privileged container can have host-root impact. Guest administrator compromise can also modify VM storage and every read-write shared mount. Use current host kernels, KVM, container runtime, WinBoat, FreeRDP, and Windows patches; keep mounts minimal; avoid secrets in the shared workspace; and run WinBoat only for trusted workloads.

## Reporting a vulnerability

Open a private GitHub security advisory for the repository when possible. Include the affected version, host/runtime details, reproduction steps, and whether the issue crosses the Linux host, shared workspace, container, Windows guest, or RDP trust boundary. Do not include credentials, private project data, or signing material.
