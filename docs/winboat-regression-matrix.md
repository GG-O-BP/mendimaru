# WinBoat lifecycle regression matrix

## Startup and disposable UEFI recovery (#129–134)

| Boundary                                                                        | Automated scenarios                                                                                                                                                                                                                 |
| ------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Backend readiness (`winboat/startup.rs`)                                        | Stopped → running → delayed health; early exited/dead; running without health timeout; already online idempotence; exit during health                                                                                               |
| Real CLI (`cli_e2e::startup_command_failure_matrix_is_bounded_and_secret_free`) | Inspect timeout/failure, unsafe API port mapping, unavailable health, exact QEMU/unknown logs, log command timeout/failure; bounded end-to-end wall time and sanitized stable codes                                                 |
| Bounded log classification (`startup_diagnostics.rs`)                           | Exact allowlist only, ANSI wrapper, byte/line limits, invalid bytes, secret-like content excluded from diagnostics                                                                                                                  |
| React (`useEnvironmentStatus`, `useWinBoatControl`)                             | Accepted ≠ success, rapid transitional polling then normal polling, sticky failure, newer external success, singleflight, stale generation, unmount cleanup and duplicate clicks                                                    |
| Full React App (`App.e2e.test.tsx`)                                             | Clock degraded/nonblocking, RDP blocking, browser installation-only blocking, Studio/project buttons, accessible diagnostic focus, native Windows presentation                                                                      |
| Actual Linux desktop (`test-tauri-e2e.mjs`)                                     | Real WebKit and Tauri IPC Start click → starting → online / startup-failed, no premature success, safe QEMU cause, WinBoat/Settings recovery actions, en/ko/ja localization                                                         |
| Disposable recovery (`nvram/tests.rs`, actual desktop IPC)                      | Fully stopped identity, explicit preview/confirmation, backup-before-retire, generated store verification, failure rollback, symlink/hardlink/ambiguous mount and collision refusals; data disk and Compose byte-for-byte unchanged |

`npm run test:e2e:coverage` checks source coverage and corroborates the startup and
recovery assertions against the actual desktop report when one exists. The Linux
CI test job runs the desktop test before this gate. These tests create temporary
host files and a fake runtime/API; they never invoke a real Docker daemon or access
a user's WinBoat storage. Recovery format evidence is intentionally narrow; see
[supported recovery and limitations](winboat-nvram-recovery.md).

Live lifecycle testing requires both `MENDIMARU_E2E_ALLOW_MUTATION=1` and
`MENDIMARU_E2E_DISPOSABLE_SNAPSHOT=<restorable-snapshot-id>`, plus an absent exact
`MENDIMARU_E2E_VERSION`. The operator must create and verify that disposable VM
snapshot first; the identifier is an explicit acknowledgement, not an automatic
snapshot creation or validation service. Hosted CI never supplies these opt-ins.
No live VM recovery is claimed by the disposable file/IPC test.

The normal Rust test suite intentionally does not require a live WinBoat guest,
Windows VM, RemoteApp connection, or Mendix account. The lifecycle fixtures in
`src-tauri/tests/support/winboat_lifecycle.rs` reconstruct only private host
state and safe contract data:

- current `4.0.0` stopped/starting/ready Runtime records;
- the exact pre-0.3.0 `3.0.0` starting record from issue #98;
- clean, dynamic, public, fixed-stale, and unrelated-user Compose baselines;
- exact and negative Mendix `.mpr.lock` shapes;
- live and orphan Linux session-keeper Unix sockets;
- path-free CLI result/error envelopes.

`winboat_lifecycle_matrix.rs` runs representative cases in the ordinary CI Rust
test job. Live WinBoat and Windows E2E remains behind the repository's explicit
live/Windows gates and never becomes a prerequisite for this matrix.

| Defect class         | Representative fixture assertion                                                                                           | Issue |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------- | ----- |
| Legacy cache reuse   | schema `3.0.0` is listed incompatible, cannot be loaded as a current session, and is auditable after explicit invalidation | #98   |
| Post-success cleanup | authoritative absence/live/unverified decision and Runtime-stop diagnostics                                                | #99   |
| Error contracts      | `runtime_session_not_found` survives sanitization with stable exit/retry semantics                                         | #100  |
| Session discovery    | `runtime list` exposes safe summaries and `runtime forget` rejects active records                                          | #101  |
| Compose baseline     | target Runtime mappings are removed from rollback baselines while system/user ports survive                                | #102  |
| Keeper hygiene       | connection-refused sockets are removed while live and untrusted entries survive                                            | #103  |

## Concurrent Runtime stop and keeper cleanup (#146)

`cli::runtime_stop_tests` runs CLI command dispatch and the real session-keeper
loop in isolated subprocesses with a registered, live RDP stand-in. Fake Compose
disconnects that client while its first recreation is held at a barrier. The
tests assert one recreation, no overlapping Compose children, restored Compose
bytes, and a final stopped record. They also inject a first-recreation failure
and verify the keeper's serialized recovery and an idempotent explicit retry.
Separate cases cover simultaneous explicit stops, a cancelled lock waiter, lock
owner process death, and refusal of symlinked, hardlinked, public, or non-file
locks. The lock unit test checks its bounded asynchronous wait.

These tests run in the ordinary Rust suite. Docker, guest health, and the RDP
client are fixtures; they do not prove behavior of a real Windows VM. Live
reproduction still requires the verified disposable snapshot described above.
On that VM, launch a keeper-owned Studio session with a linked Runtime, issue one
explicit Runtime stop, and verify both processes exit, Compose is restored, the
configured container name is running, and the Runtime record is stopped. A
second stop must succeed without another recreation.

## Contract schema upgrade checklist

Whenever `CONTRACT_SCHEMA_VERSION`, a runtime schema, or a persisted WinBoat
record layout changes, add a PR item for each step below:

1. Add or update a legacy record fixture using the exact public field shape
   from the incident or migration (without credentials or host paths).
2. Assert that active-port scanning and ID loading give the same compatibility
   answer for current, legacy, stopped, corrupted, and identity-mismatched
   records.
3. Keep incompatible-record invalidation auditable and test that a current
   session can still be created after the legacy state is present.
4. Exercise `runtime list` and `runtime forget` against current, active,
   stopped/failed, incompatible, and already-invalidated records.
5. Re-run the post-success failure, Compose baseline, lock cleanup, keeper
   socket, and CLI error-code cases in this matrix.
6. Update this table and the contract docs in the same change.

Fixture builders must not bypass or weaken schema validation, file-type checks,
permissions, bounded reads, hashes, or process identity checks. They also must
not include real host paths, credentials, command lines, or remote output.
