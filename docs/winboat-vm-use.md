# Shared WinBoat use and lifecycle exclusion

Linux WinBoat browser tests can protect the VM for the **whole command**, including
metadata lookup, browser startup, suite execution, and artifact collection. A
lifecycle operation must obtain exclusive use before changing Compose, recreating
the container, or recovering UEFI. This is the foundation from #150. Session participation and cleanup
ownership (#151) now builds on it — `browser session prepare/finalize` and
`browser test --shared-session-id` hold shared use, and a finalize stop takes
exclusive use through the normal Runtime stop path. UI arbitration (#152) and
the parallel suite runner (#155) remain separate work. Multi-project port,
Compose, and data ownership (#153) is defined in
[Multi-project Runtime ownership](winboat-multi-runtime.md).
[Environment change observation](browser-environment-observation.md)
now supplies retrospective evidence for #154.
now supplies retrospective evidence for #154.

## Using it

A `browser test --runtime-session-id` targeting a WinBoat Runtime automatically
holds shared use. Independent read-only browser commands can run concurrently:

```bash
mendimaru browser test --runtime-session-id runtime_<id> \
  --suite-path read-only.browser.json --timeout-seconds 300 --json
```

For a plain URL in the configured WinBoat, opt in explicitly. This also works
with a separate config/cache that has no copy of the owner's Runtime record:

```bash
mendimaru browser test --base-url http://127.0.0.1:8080/ \
  --winboat-use --suite-path read-only.browser.json --timeout-seconds 300 --json
```

`--winboat-use` requires the Linux WinBoat backend and `--base-url`. It protects
the VM named by that configuration; it does not infer a VM from a URL or enable
asset mirroring. A plain external URL without this flag does not join VM use.
Portable Runtime behavior is unchanged. A lease is command-scoped, not permanent
ownership of a running Studio process or server. Stop between commands is still
possible. External test tools do not automatically participate.

Read-only suites may share an app. Concurrent writes still need independent
accounts, data, and files; the lease does not isolate application state.

## Identity and generations

Within one Linux UID and host filesystem namespace, the management identity is
`(container runtime, explicit Compose container_name)`. An existing Compose file
must identify exactly one WinBoat service, and its name must match configuration.
Ambiguous/mismatched Compose identities are refused. Configuration/cache/worktree
paths and Compose filenames do not enter the key. When Compose is absent, the
configured management name reserves that identity; existing lifecycle preconditions
still apply before any mutation.

The fixed `/tmp/mendimaru-vm-use-<uid>` directory is independent of `TMPDIR`, XDG,
and Mendimaru cache overrides. Names are SHA-256 encoded in lock filenames. Docker
contexts, executable paths, and daemon endpoint aliases are intentionally excluded:
using aliases cannot split a lease. This conservatively excludes same-named VMs
on different daemons too. Use one stable management name; a transient container ID
is not a management name. Changing Docker context or renaming a managed VM while
in use is an external control operation.

The lock inode survives container absence and recreation. Its 16-byte opaque
transaction-generation token changes on each new exclusive acquisition, before
work starts, including failed or cancelled attempts. Nested owner calls reuse the
same token. This separates a stable management identity from lifecycle generations;
it is not proof that a container was successfully recreated. The actual Docker
container ID is a separate observed incarnation. Tokens are diagnostic hints,
never ownership records, and cannot themselves detect external Docker changes; the browser observer samples
actual container, Compose and port evidence separately.

There are no persisted PID-owner records or stale-owner deletion heuristics.
Each scoped owner verifies its PID and `/proc/self/stat` start time before reuse.
The kernel's open-file `flock` ownership decides whether readers/writers remain
live. PID reuse, stale token bytes, and reboot cannot authorize eviction. Crash or
SIGKILL closes the descriptors; the inode is retained for existing waiters and
future processes. Descriptors are close-on-exec and do not grant unrelated children
mutation authority. No participant is killed as a cleanup mechanism.

## Modes, waiting, and lock order

| Operation                                                                                 | Use                                                         |
| ----------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| WinBoat Runtime status/wait/url and complete linked browser test                          | Shared                                                      |
| Plain URL browser test with `--winboat-use`                                               | Shared                                                      |
| Runtime start/stop, Studio launch (including forwarding preparation/recovery), VM startup | Exclusive                                                   |
| Settings Compose save/rollback/recreation                                                 | Exclusive for both old and new VM identities                |
| UEFI preview/recovery/restore                                                             | Exclusive, plus existing UEFI maintenance exclusion         |
| Local cached listings/logs and environment diagnostics                                    | Observations only; do not reserve an app execution interval |

Acquisition uses nonblocking kernel attempts with asynchronous polling every
25 ms for **at most 3 seconds per VM**. The command's shorter timeout or cancellation
wins. Exhaustion returns a structured `precondition_failed`, `retryable: true`, and
the exact path-free message `WinBoat VM is busy with another participant; retry
after its use or lifecycle operation finishes`. Exit status is 1. Lock trust
failures and forbidden upgrades return nonretryable preconditions with separate
allowlisted messages. GUI paths retain these safe messages too.

There is no FIFO or writer-priority guarantee: incoming readers may run while a
writer waits. Writers receive a bounded busy result instead of waiting indefinitely;
retry after participants finish. Cancellation removes only that caller's descriptor
and never releases someone else's use. Cleanup denied by a reader leaves forwarding
intact; explicitly retry `runtime stop` after shared use ends. The lease does not
undo Docker requests already accepted by the daemon when a process crashes.

The new transaction order is VM use → existing maintenance lock → Runtime-stop
lock → Compose/guest work and recovery → final record write. Existing operation
trackers may already own a shared maintenance guard; maintenance acquisitions are
always immediate try-locks, so they never wait while holding a VM lease. The VM
wait remains bounded. Settings takes distinct VM identities in runtime/name order,
deduplicates them, and retains both through rollback. Two-VM acquisition therefore
has a maximum six-second wait, still inside the CLI/GUI caller's lifetime.

Only nested calls in the same async owner scope reuse an exclusive lease. Spawned
tasks and other processes acquire independently. Shared → exclusive upgrade is
rejected immediately. Finish the complete shared scope, then acquire exclusive
use and reload/revalidate state. The Runtime stop path still reloads its record
after acquiring its original stop lock; #146 idempotence and recovery remain.

## Trust and compatibility

The directory must be owned by the current UID, mode 0700, and reached through
no-follow directory handles. Lock files must be regular, owned by that UID, have
one link, and no group/other permissions. Symlinks, hardlinks, FIFOs, directories,
and public files are refused before mutation. Locks are never unlinked, including
on timeout or crash. The bounded generation token contains no credentials, command
lines, paths, or guest output; CLI diagnostics use only exact static reasons.

This is **same-user advisory coordination**. Other UIDs, filesystem namespaces,
older Mendimaru versions, arbitrary Docker/WinBoat controllers, and malicious code
running as the same UID are outside the boundary. They can change the VM without
participating. All cooperating clients must run the updated code. It is not a
Docker permission mechanism or a claim of cross-user/external-controller exclusion.

Runtime/session schemas stay at 4.0.0; existing records are not rewritten to adopt
leases. The new private generation file has no owner record migration: empty or
stale bytes do not affect liveness, and the next exclusive holder writes 16 bytes.
The ordinary legacy/current schema, Compose rollback, keeper, and maintenance
regression matrix remains required.

## Verification

`winboat::vm_use::linux::tests` uses real independent processes with distinct
config/cache/Compose paths to test parallel readers, exclusive writers, writer
competition, deadlines, cancelled waiters, owner SIGKILL, simulated PID/start reuse,
and stale bytes. Lock trust tests cover symlink/hardlink/public/non-file entries.
The generation changes across exclusive owners without replacing the inode.

`cli::runtime_stop_tests` runs actual Chromium with a keeper-linked HTTP fixture.
Both Runtime-linked tests and plain URL tests in another config/cache are covered.
While a browser navigation is held at a barrier, independent Runtime stop/start and
recreate requests must return busy with no Compose changes or Docker recreation.
After the browser completes, confirmed Studio exit can clean up once. Existing
#146/#147/#148 and schema-upgrade tests also run. Docker, RDP, and Windows are
fixtures here; this is not an actual-VM lifecycle claim. Actual VM lifecycle tests
still require the disposable snapshot in the regression matrix.
