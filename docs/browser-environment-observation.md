# Browser environment generations (Linux + WinBoat)

`browser test --runtime-session-id` and `browser test --base-url ... --winboat-use`
observe the configured VM throughout a browser run. A changed or unavailable
observation interrupts the current test, skips remaining tests, and gives the run
exit code 1. The original browser failure, completed steps and diagnostic artifacts
are retained. `summary.json`, the CLI JSON summary and `artifact-manifest.json`
include the same `environment` report. A final observation can fail a run even if
all its browser assertions passed; `failed` counts test cases; a final environment event may fail a run
without increasing that count. Consumers must check `outcome` and `environment.comparable`.

```bash
mendimaru browser test --runtime-session-id runtime_<id> \
  --suite-path read-only.browser.json \
  --build-marker /path/to/build-generation --timeout-seconds 300 --json
```

## What is observed

| Component       | Read-only evidence                                                                                          | Classification after a change |
| --------------- | ----------------------------------------------------------------------------------------------------------- | ----------------------------- |
| Managed VM      | #150 management-key hash and generation bytes from the retained lease descriptor                            | `environment-changed`         |
| Container       | Docker/Podman inspect container ID and running state                                                        | `environment-changed`         |
| Compose         | SHA-256 of the configured Compose file, at most 4 MiB                                                       | `environment-changed`         |
| Published ports | Sorted, deduplicated guest port/protocol and host port; normalized bind-address hash                        | `environment-changed`         |
| Runtime         | Digest of stored session ID, start time, ports and linked Studio identity; stopped/missing records detected | `session-lost`                |
| Studio          | Existing registered owner or keeper IPC: PID **and** process start time, plus observed running state        | `session-lost`                |
| Build           | Explicit marker content/metadata digest and file-watch generation                                           | `build-changed`               |

The Runtime observation does not refresh/write the record or probe Windows.
Studio observation uses #148's authenticated owner path, never a new RDP session.
A lost/unknown owner is evidence of session observation loss, not proof of Studio
process death. No observer invokes stop, Compose up, recovery or teardown.
Unsuccessful observations are `observation-unavailable` when no prior identity
exists; disappearance of a previously observed identity is classified by its
component. No exception, command output, credentials, project name, host path,
process command line or raw Compose content enters these observations.

A snapshot has a host UTC completion timestamp. Fields are sampled within an
interval, not in an atomic cross-process transaction. Polling is one second plus
observation duration, with a two-second Docker policy and two-second keeper budget
in parallel. The private loopback endpoint has an unguessable path, serves only
observations, allows one bounded request at a time, and is dropped with the run.
The runner applies a six-second request deadline and 512 KiB response limit.
Published bindings are limited to 256. The report retains the baseline, latest
snapshot and **first** event for each of seven components. Reverting an environment
change cannot erase its first evidence. This bounds memory/artifact growth for
long runs. Inspection/schema/size/watch errors never assert comparability.

## Build preparation and expected rebuilds

`--build-marker` is a direct regular host file of at most **64 KiB**, accessible
from Linux. The build owner must update or atomically replace it **on every build
or watch emission**, including a rebuild producing identical content. Use a build
pipeline generation marker, not an arbitrary unchanged source file. This explicit
contract is needed because neither a Studio PID nor a successful HTTP response
identifies the compiled application. Mendimaru does not create a marker or claim
that an unrelated file proves a build.

The observer hashes only that marker and bounded Compose content. It watches the
marker parent nonrecursively, so atomic rename and identical-content replacement
invalidate a run. File inode/device/size/mtime/ctime participate in the marker
digest. A watcher error or rescan request invalidates observation; the observer
never silently reinstalls a watcher and reuses the old preparation. Parent
replacement also changes file identity. There is no repeated project tree hash.

Expected rebuild procedure: finish/detach the previous browser run, complete the
new build, update the marker, then issue a **new** browser command. Each command
has distinct browser session and preparation IDs and retains its own evidence.
There is no automatic retry or combining results across preparation IDs. An
external retry orchestrator must preserve the old run and explicitly link its new
run; #151 can reuse these preparation/snapshot contracts without depending on the
observer to implement leases or ownership.

Without a marker the Build component is listed in `missing`; with a plain URL,
Runtime and Studio are also missing. Such a browser suite may pass but its report
has `comparable: false`. Supplying an unreadable/invalid marker interrupts the run.
Full comparability requires every component observed and no events. Legacy
browser records without `environment` remain readable; they carry no environment
comparability claim. This is an additive browser report field, with no Runtime or
Studio record/schema-version migration.

## Limits and separate gates

Detection is retrospective. External Docker/WinBoat controllers do not obey the
advisory lease automatically; this observer does not prevent them from acting.
Snapshots can miss a change and restoration entirely between samples. The marker
watcher narrows that gap for build writes but is not a universal guest filesystem
monitor. All reports say `actor: "unknown"`: temporal correlation alone cannot
attribute a change to an external controller or to Mendimaru/product code.

Ordinary tests cover classification, same-PID/new-start identity, marker replacement,
watch failure, bounded/redacted inspection, endpoint failure and compatibility.
`npm run test:browser:environment:fixtures` separately performs intentional external
changes against keeper/Docker fixtures while actual Chromium is active, including
container, port, Runtime and identical-content build replacement. Fixtures prove
the report/artifact/CLI behavior, not actual Windows lifecycle behavior.

Actual VM changes require the regression matrix's explicit mutation opt-in and a
verified, restorable disposable snapshot. Reserve that VM exclusively for the
whole gate; independent normal test workers must not join it. Read-only baseline
checks, external changes, and their artifact evidence belong in separate runs.
