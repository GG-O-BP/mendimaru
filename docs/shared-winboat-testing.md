# Shared Linux + WinBoat test sessions (#149)

Use one owner to prepare a real Studio Run Locally app, then attach independent
browser workers to that fixed preparation. All cooperating clients must use the
same current Mendimaru version, Linux UID, and host filesystem namespace.
Windows native Studio automation is outside this support claim.

```bash
# Once Studio F5 is ready and the build owner has written its generation marker:
mendimaru browser session prepare --runtime-session-id runtime_<id> \
  --build-marker /absolute/build-generation --json

# Independent read suites can run concurrently, including separate CLI processes.
mendimaru browser test --shared-session-id shared_<id> \
  --suite-path app-reads.browser.json --workers 2 --asset-mirror off --json

# Run after workers have joined; this drains participants before finalizing.
mendimaru browser session finalize --shared-session-id shared_<id> --json
```

The preparation records VM/container/Compose/ports, Runtime, Studio PID/start,
and build-marker observations. Each participant compares against that original
preparation and reports the **same `preparationId`**. Changes between commands
are invalidations too: a worker must not silently adopt a new container or build.
A missing build marker makes comparisons incomplete; it never proves a fixed
build. Update the marker on every build/watch emission, finish the old session,
and prepare again. Watch counters belong to each observer; file content and
metadata detect replacement between preparation and attachment.

The Runtime record and authenticated keeper must be accessible in the owner's
cache. A participant with an unrelated cache cannot silently fall back to an
unobserved URL. Separate-cache plain-URL callers can still coordinate VM use
with `--winboat-use`, with the documented incomplete Runtime/Studio observations.

## Coordination boundaries

| Activity                                         | Coordination                                                                                                    |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| Stable page and asset reads                      | Shared VM use and shared app-data lock; independent contexts and artifacts                                      |
| Data writes, write-back, import/export           | VM-wide exclusive app-data lock for the entire CLI suite; undeclared suites also take this lock                 |
| Verified distinct data scopes inside one suite   | Existing bounded scheduler may overlap them; author must establish accounts/data/file isolation                 |
| Same Studio actions                              | Existing keeper FIFO and desktop foreground exclusion; perform build-affecting actions before preparing workers |
| Runtime stop/start, Compose changes, VM recovery | Exclusive VM use; active participants cause a bounded retryable busy refusal                                    |
| External Docker/WinBoat controllers              | Outside advisory exclusion; observed changes invalidate results retrospectively                                 |

The app-data lock adds coordination **between CLI processes**, including callers
with different caches. It conservatively serializes whole write suites even
when their declarations claim distinct scopes. A write cannot overlap a reader
through a second CLI. Lock order is VM use, app data, then artifact store.
App-data contention waits at most three seconds and returns a path-free retryable
precondition. Cancellation/crash releases only that process's kernel ownership;
there is no PID eviction or shared-to-exclusive lock upgrade.

Browser failure, cancellation and worker death never authorize Runtime/Studio
cleanup. The owner finalizes after participation drains. Default `keep` preserves
an existing app; `stop` requires the explicit prepare-time `--owns-runtime` claim.
Wrong-VM finalization is refused before changing session state. A crashed finalizer
can be retried, and completed finalization is idempotent.

Multi-project ports and Compose ownership follow
[the multi-Runtime contract](winboat-multi-runtime.md). A browser context does not
isolate a server database. Port preparation requiring VM recreation is refused
while another Runtime is active. The single connected Studio RemoteApp limit and
single-slot UI helper remain in force.

## Upgrade behavior

Public Runtime and Studio schemas remain `5.0.0`. The private shared-session
record adds an optional validated `preparation` report with a 512 KiB record
limit. Records from before this change remain readable and finalizable; attaching
to one fails with an explicit instruction to prepare a fresh verified session.
They are never silently upgraded into a comparable preparation. Legacy Runtime
invalidation, current-session discovery, Compose rollback, keeper cleanup, and
post-success recovery remain covered by the ordinary Rust suite.

## Integration gate

`npm run test:browser:shared:live` requires an **installed** Linux binary, a real
keeper-linked Studio F5 Runtime, an explicitly verified restorable disposable
snapshot, and the mutation opt-in. It refuses a Cargo-target executable, runner
or Node override, and a VM name outside the isolated `Mendimaru149` namespace.
The operator supplies a read-only suite with at least two asserted app cases.
Give those cases enough work to remain active across the contention barriers.

```bash
MENDIMARU_CONFIG_DIR=/absolute/lab/config \
MENDIMARU_CACHE_DIR=/absolute/lab/cache \
MENDIMARU_E2E_BINARY=/absolute/installed/usr/bin/mendimaru \
MENDIMARU_E2E_RUNTIME_SESSION_ID=runtime_<id> \
MENDIMARU_E2E_KEEPER_PID=12345 \
MENDIMARU_E2E_BROWSER_SUITE=/absolute/app-reads.browser.json \
MENDIMARU_E2E_BUILD_MARKER=/absolute/build-generation \
MENDIMARU_E2E_SHARED_REPORT=/absolute/evidence/shared.json \
MENDIMARU_E2E_ALLOW_MUTATION=1 \
MENDIMARU_E2E_DISPOSABLE_SNAPSHOT=verified-restorable-snapshot-id \
  npm run test:browser:shared:live
```

The gate exercises two independent CLI processes with two scheduler lanes each,
Runtime-stop, conflicting-writer and UI-mutation exclusion, artifact commit,
two-run retention and concurrent export, an intentionally failing worker, SIGTERM and SIGKILL,
finalizer crash/recovery, late-attach refusal, and owner cleanup after the last
participant. It compares container/ports/Compose, Studio/keeper/FreeRDP identities
between scenarios and requires unmodified Chromium, complete comparable
observations, and actual assertions. Duplicate cleanup must not recreate the VM.
Raw private environment files are never publication artifacts. The safe report
records binary and suite hashes and scenario outcomes; failed runs retain their
failure outcome. Ordinary process/Chromium fixtures and gate-helper tests remain
separate from this live acceptance evidence.

The release parity gate can also borrow a prepared session. Set
`MENDIMARU_STUDIO_PARITY_SHARED_SESSION_ID=shared_<id>` in the self-hosted
runner's environment alongside its owner config/cache. It verifies ordinary
and assisted runs against the same comparable preparation and includes both
environment reports in its evidence. It never starts, stops, or finalizes that
borrowed Runtime, even after a failed assertion; the owner handles cleanup.
Its existing suite requirement for a state change and a value assertion still
applies. Omitting the setting retains the original standalone gate flow.
