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
disconnects that client while its first recreation is held at a barrier, and
the fixture supplies a newer authenticated Studio-exit report. The
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

## Keeper socket preflight and failed launch cleanup (#147)

The ordinary Rust suite binds real Linux Unix sockets at 107, 108, and 156
pathname bytes and checks multibyte cache names, repeated preflight cleanup, and
the exact diagnostic allowlist. A real CLI subprocess test checks both plain
and project launches with long cache paths and untrusted socket directories:
the error is actionable and path-free, and no guest command, launch history, or
Runtime state is created. A short-cache control reaches the fake Docker command.

`cli::runtime_stop_tests` also injects a final socket collision after successful
preflight/launch and a missing caller acknowledgement. Registered RDP stand-ins
receive stop requests and exit, the completed launch becomes interrupted, linked
Runtime forwarding is restored with one serialized Compose recreation, untrusted
files survive, and the same socket path can be bound on retry. These isolated
fixtures do not claim a live Studio Pro or VM reproduction.

## Safe browser and Runtime observation (#148)

The ordinary Rust suite runs the real keeper loop, Unix IPC, CLI dispatch, and
Chromium against an HTTP fixture with a linked Studio session. Repeated browser
tests and Runtime status/wait/url/logs/list leave the client, Studio identity,
Compose bytes, and published-port fixture unchanged, with zero RDP launches or
Compose recreations. The local GUI-owner path is covered separately. Missing,
timed-out, malformed, wrong-session, wrong-schema, invalid-version, and stopped
metadata fail with a bounded, retryable, path-free browser diagnostic.

RDP loss plus missing/tampered reports preserves the Runtime across automatic
keeper ticks. A later authenticated Studio-exit report permits one cleanup.
Readiness timeout cases cover both available and missing owner metadata without
guest diagnostics or teardown. Explicit guest discovery is still distinct from
owner observation; the #99 authoritative-absence checks remain required.

For an **already-running actual keeper-linked Studio F5 session**, run:

```bash
MENDIMARU_CONFIG_DIR=/absolute/test/config \
MENDIMARU_CACHE_DIR=/absolute/test/cache \
MENDIMARU_E2E_BINARY=/absolute/current/mendimaru \
MENDIMARU_E2E_RUNTIME_SESSION_ID=runtime_0123456789abcdef0123456789abcdef \
MENDIMARU_E2E_KEEPER_PID=12345 \
MENDIMARU_E2E_BROWSER_SUITE=/absolute/read-only-smoke.browser.json \
node scripts/test-browser-winboat-live.mjs
```

Choose a read-only suite that asserts the actual app's heading or another stable
element. The gate requires HTTP-ready `studio-run-locally` mode and trusted keeper
metadata. It executes two browser runs with strict console/network checks and ten
Runtime status reads, sampling before/during/after and for three seconds afterward.
It compares the actual container ID/status, Compose SHA-256, all published ports,
Studio PID/start identity from the authenticated session owner, keeper PID/start
identity, and current-user FreeRDP PID/parent/start identities. Samples run at
500 ms intervals plus command duration; shorter-lived processes can escape this
sampling, so the fixture's zero-launch assertion remains complementary evidence.
The Studio identity is owner-reported, not an independent Windows process query.

The gate emits a JSON report without process arguments, project paths, credentials,
or Compose content. It never starts/stops Studio or restores Compose. Preparing
or tearing down a live test session must follow the disposable-snapshot rules
above; the existing-session observer itself needs no mutation opt-in. Missing
prerequisites fail rather than silently substituting an HTTP-only or unlinked
session. Automated gate fixtures do not count as an actual VM run.

The [2026-09-15 verification record](issue-148-verification.md) includes an actual
restored-VM browser run, missing-owner diagnostic, RDP loss, authenticated Studio
stop, and original-environment restoration, with explicit app-test limitations.

## Shared test session ownership (#151)

`winboat::test_session::tests` covers the registry state machine, trust
boundaries (symlinked/public/oversized/drifted records fail closed), duplicate
transitions, wrong-VM attach refusal, kernel-owned participation liveness, and
finalize-lock serialization with kernel recovery. `cli::browser_session_tests`
runs real CLI dispatch and real Chromium participants against the keeper-linked
HTTP fixture: two workers join one prepared session and pass; a SIGKILLed
worker's participation disappears while the other worker, container, Compose,
keeper, and Studio identity stay intact; finalize with `keep` performs no
Runtime action and is idempotent on duplicates; an owner-claimed session stops
the Runtime exactly once through the normal serialized stop path after an
authenticated Studio-exit report; a drain timeout is retryable and returns the
session to `ready`; attach during `preparing`/`finalizing`/`finalized` is an
explicit refusal; a crashed finalizer is recovered by re-running finalize; and
the #150 exclusion holds in both directions (participants block lifecycle,
exclusive reservations block new participants). Participants open no RDP
connection and perform no Runtime status read: the fixture's `unexpected-rdp`
marker and untouched Compose/inspect fixtures assert this.

Docker, RDP, guest health, and keeper internals remain fixtures here; this is
not an actual-VM lifecycle claim. Real F5 multi-worker participation still
requires the disposable snapshot workflow above, with the owner preparing from
a real `runtime start`/F5 session and at least two worker processes.

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

## Frontend diagnosis without asset bypass (#144)

The keeper observation fixture also executes `browser frontend-health`, checks
Studio/HTTP/frontend fields separately and requires `assetBypass: false`.
The client, Studio identity, Compose bytes and port inspection remain unchanged,
with zero RDP launches or Compose recreations. The CLI/Chromium frontend matrix
uses ordinary network resolution, including a failing shared UNC import; it
does not use the existing browser-test mirror as frontend-health evidence.
Runtime-linked and explicit `--winboat-use` URL diagnoses also hold browser
navigation at a barrier while concurrent stop/start/recreate requests must
return a busy precondition, preserving Studio, Compose and ports.

## VM use across processes (#150)

The ordinary Rust suite includes real subprocess reader/writer, different-cache/
Compose-copy identity, writer competition, timeout/cancel/SIGKILL, simulated PID
reuse, stale-generation, forbidden upgrade, and file-trust tests. Real Chromium fixtures for linked Runtime and plain URL targets in another cache
hold navigation open while Runtime start/stop/recreate must return a
structured busy precondition and preserve Compose, container inspection, and the
keeper; after release, one cleanup succeeds. These fixtures do not mutate a real VM.

No Runtime/session record fields or contract schemas change. Existing 3.0.0 legacy
invalidation and 4.0.0 creation/discovery, authenticated post-success recovery,
Compose rollback, maintenance locks, and keeper socket regressions remain in the
ordinary suite. The new 16-byte lock-generation hint never determines process
liveness, and empty/stale contents cannot evict a live owner. See the
[identity, trust, and acquisition policy](winboat-vm-use.md).

## Browser environment change evidence (#154)

WinBoat browser runs now record bounded environment observations and interrupt on
changes. See [environment generations](browser-environment-observation.md) for
`--build-marker`, JSON comparability, preparation boundaries, observation limits,
and the separate external-change fixture and disposable-VM gates.

## Bounded parallel test execution (#155)

`node --test scripts/browser-parallel.node-test.mjs` locks the scheduler policy
without a browser, a VM, or a filesystem: the closed and bounded worker policy
(1–8 lanes, 1000–1800000 ms, unknown keys refused) and the single-worker default
with no deadline; closed resource/scope/isolation validation, including
`verified` without a scope and a scope on a non-`data-write` test; refusal of a
`vm-lifecycle` test for a shared-session participant; head-of-line admission in
declaration order; the exclusive barrier that later tests never overtake;
reader sharing, unverified-write serialization, same-scope exclusion and
distinct-scope overlap; one Studio UI action at a time; per-lane failure,
deadline, and cancellation isolation; environment-change invalidation of both
running and pending tests; deterministic result order under out-of-order
completion; and worker-limit resolution against declared capacity, CPU, and
memory.

`npm run test:browser` adds the real-Chromium parallel scenario against the same
fixture app: declared lanes overlap, each test keeps its own context and
`test-NNN` artifacts, and the summary, manifest, and report stay in declaration
order. The Rust CLI and summary validation tests cover `--workers` and
`--worker-timeout-ms` parsing and range rejection and the `concurrency` record
they produce.

These remain fixtures and a single prepared Runtime; they are not a live VM
claim. The following still require the real Linux+WinBoat gate under the
disposable-snapshot and opt-in rules above:

- one `browser session prepare`, two or more concurrent independent
  verifications, and cleanup only after the last participant leaves (#151);
- exclusive reservation of the #146 concurrent-stop and #148 connection-loss
  reproductions, so no ordinary worker runs beside them;
- artifact commit and pruning racing a concurrent read of the same cache.
