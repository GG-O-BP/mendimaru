# WinBoat coordination locks (#225)

Mendimaru serializes three kinds of work against one WinBoat VM across processes:

| Lock         | Holders                                                                                             | Mode                               |
| ------------ | --------------------------------------------------------------------------------------------------- | ---------------------------------- |
| Maintenance  | Studio install/uninstall/launch trackers, session actions, VM startup, Runtime start/stop, settings | Shared, immediate try-lock         |
| Maintenance  | UEFI preview/recovery/restore                                                                       | Exclusive, immediate try-lock      |
| Runtime stop | Runtime stop through the final record write                                                         | Exclusive, bounded wait (≤ 1 hour) |

These locks sit inside the existing [VM use](winboat-vm-use.md) transaction order:
VM use → maintenance → Runtime stop → Compose/guest work → final record write.
Maintenance acquisitions never wait, so they never wait while holding a VM lease.

## Location

Before #225 both locks were created inside the WinBoat-managed Compose directory
(`~/.winboat/.mendimaru-maintenance.lock`, `.mendimaru-runtime-stop.lock`) and
that directory had to pass the strict NVRAM storage checks (owned by the current
user and **not** group/other writable). WinBoat creates `~/.winboat` with the
user's umask, so a normal `umask 0002` host gets a `775` directory and every
ordinary operation failed before Windows was even contacted.

The locks now live in a fixed, Mendimaru-owned private namespace:

```text
/tmp/mendimaru-vm-coordination-<uid>/                 0700, owned by <uid>
  <vm-identity>.maintenance.lock                        0600, regular, one link
  <vm-identity>.runtime-stop.lock                       0600, regular, one link
```

- `<vm-identity>` is the same SHA-256 management identity as VM use: container
  runtime plus the explicit Compose `container_name`, which must match the
  configuration. Compose paths, worktrees, `MENDIMARU_CONFIG_DIR`,
  `MENDIMARU_CACHE_DIR`, `TMPDIR` and XDG overrides cannot split protection of one VM.
- Every path component is opened with no-follow directory handles. The namespace
  must be owned by the current UID with no group/other bits; lock files must be
  regular, owned by that UID, have exactly one link and no group/other bits.
  Symlinks, hardlinks, FIFOs, directories, public files and foreign owners are
  refused before any lock is taken. Mendimaru never repairs or deletes them.
- Lock files are never unlinked, including on timeout, cancellation or crash.
  The kernel's `flock` ownership is authoritative; closing the descriptor (also
  on SIGKILL) releases it.

The Compose directory permissions are no longer a precondition for ordinary work.
The security checks for data Mendimaru actually modifies are unchanged: UEFI
recovery still requires a direct, user-owned, non-group/other-writable `/storage`
bind mount and variable store (see [UEFI recovery](winboat-nvram-recovery.md)),
and Compose edits keep their own direct-file and revision checks.

## Mixed versions

An older Mendimaru (≤ 0.6.x) only coordinates through the legacy file in the
Compose directory, and only when that directory and file pass its trust checks.
The current version therefore also takes the legacy lock, in the same mode and
after the private lock, **exactly when an older version could hold it**:

| Legacy location                                                       | Older version           | Current version                                            |
| --------------------------------------------------------------------- | ----------------------- | ---------------------------------------------------------- |
| Compose directory and legacy file pass the original checks (e.g. 755) | Locks the legacy file   | Locks private **and** legacy; legacy busy → refuses        |
| Compose directory refused (e.g. 775, symlinked ancestor, foreign)     | Refuses every operation | Private lock only; nothing can overlap                     |
| Legacy file refused (e.g. 640, symlink, hardlink, directory)          | Refuses every operation | Private lock only; the entry is neither followed nor fixed |
| Unexpected I/O error on the legacy location                           | —                       | Refuses (fail closed)                                      |

There is no best-effort dual locking: when the legacy lock is joined and busy,
the current version reports the busy cause and does not proceed. A Runtime stop
waits for both locks within its bounded deadline. Running only current versions
makes the legacy file irrelevant; it is retained for compatibility and never
deleted. Changing the Compose directory permissions while an older version is
running is an external change outside this advisory boundary.

## Causes and messages

Every failure keeps its cause up to the GUI and CLI. All are structured
`precondition_failed` errors with the capability of the requested action
(`studio.install`, `studio.uninstall`, `studio.start`, `studio.stop`,
`studio.status`, `runtime.start`, `runtime.stop`, `studio.detect` for
`env ensure`, ...). The GUI shows a localized (ko-KR/en-US/ja-JP) message; the
CLI keeps exactly one path-free reason per cause:

| Cause                         | Retryable | CLI message                                                                                                            |
| ----------------------------- | --------- | ---------------------------------------------------------------------------------------------------------------------- |
| Busy (another operation/UEFI) | yes       | `another Mendimaru operation or UEFI recovery is using this WinBoat VM; retry after it finishes`                       |
| Untrusted private lock        | no        | `the private Mendimaru coordination lock failed its ownership or permission check; review the environment diagnostics` |
| Other I/O failure             | no        | `the private Mendimaru coordination lock could not be opened or locked`                                                |
| Compose names another VM      | no        | `the WinBoat Compose file does not identify the configured VM; check the Compose path and container name`              |
| Runtime stop wait expired     | yes       | `another Runtime stop for this WinBoat VM did not finish before the deadline; retry after it finishes`                 |

A permission problem is never reported as "another operation is running", and a
failure before any start command is never reported as "the Windows startup
command failed". Exit status remains `1`.

## Diagnostics

`env status`, the Settings diagnostics and the exported report include the
`coordination-lock` check. It opens and verifies the same private (and, where
applicable, legacy) lock files every coordinated operation needs, without
holding any lock, so it never reserves the VM or delays UEFI recovery. A failure
carries `coordination-lock-untrusted`, `coordination-lock-unavailable` or
`coordination-identity-mismatch`, is listed in `readiness.blockingChecks` and
clears `ready`, so the readiness flags match what operations will actually do.
This preflight is independent of startup attempts, so a lock failure that occurs
before a startup attempt is recorded is still visible.

## Verification

`winboat::maintenance::tests` covers `755`/`775`/`700` Compose directories with a
`664` Compose file, private `600`/`640` files, symlinked/hardlinked/FIFO/directory
entries, public or symlinked namespaces, exclusive vs shared exclusion across
different Compose paths of one VM, older versions holding the legacy lock
(shared, exclusive and Runtime stop), refused legacy locations that are neither
joined nor followed, identity mismatch, and distinct causes/messages. Real child
processes with separate config/cache/TMPDIR prove UEFI exclusion, SIGKILL release
and Runtime stop serialization across processes. `cli::runtime_stop_tests` runs
real CLI stops against both a legacy owner and a current owner and checks the
exact untrusted reason for the private lock.
