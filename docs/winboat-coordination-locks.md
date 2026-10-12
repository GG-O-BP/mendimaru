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
  refused before any lock is taken. No automatic repair or deletion occurs.
  Owner-verified permissions may be repaired only through the explicit preview below.
- Lock files are never unlinked, including on timeout, cancellation or crash.
  The kernel's `flock` ownership is authoritative; closing the descriptor (also
  on SIGKILL) releases it.

The Compose directory permissions are no longer a precondition for ordinary work.
The security checks for data Mendimaru actually modifies are unchanged: UEFI
recovery still requires a direct, user-owned, non-group/other-writable `/storage`
bind mount and variable store (see [UEFI recovery](winboat-nvram-recovery.md)),
and Compose edits keep their own direct-file and revision checks.

## Mixed versions

Mendimaru versions before #226 used legacy files in the Compose directory. The
current version always joins an existing legacy regular inode, even when the
parent changed to 775 after acquisition. A trusted legacy file is opened without
creation; busy ownership refuses the operation. Existing unsafe modes, symlinks,
hardlinks, non-files, unknown owners or I/O errors fail closed. The file is never
unlinked or silently ignored.

If the legacy file is absent, it is created only in a directory passing the
original non-group/other-writable policy. A normal 775 folder with no legacy file
uses just the private lock. Compose symlinks, foreign owners and world-writable
paths are not this normal case and are rejected. VM identity/private coordination
remains the source of truth across current clients with different Compose paths.

## Causes and messages

Every failure keeps its cause up to the GUI and CLI. Contract v6 supplies distinct
`coordination_*` codes with the capability of the requested action
(`studio.install`, `studio.uninstall`, `studio.start`, `studio.stop`,
`studio.status`, `runtime.start`, `runtime.stop`, `studio.detect` for
`env ensure`, `environment.configure` for settings and setup). The GUI shows a localized (ko-KR/en-US/ja-JP) message; the
CLI keeps exactly one path-free reason per cause:

| Cause                         | Retryable | CLI message                                                                                                                |
| ----------------------------- | --------- | -------------------------------------------------------------------------------------------------------------------------- |
| Busy (another operation/UEFI) | yes       | `another Mendimaru operation or UEFI recovery is using this WinBoat VM; retry after it finishes`                           |
| Untrusted private lock        | no        | `a Mendimaru coordination entry failed its ownership, direct-path or permission check; review the environment diagnostics` |
| Other I/O failure             | no        | `the private Mendimaru coordination lock could not be opened or locked`                                                    |
| Compose names another VM      | no        | `the WinBoat Compose file does not identify the configured VM; check the Compose path and container name`                  |
| Runtime stop wait expired     | yes       | `another Runtime stop for this WinBoat VM did not finish before the deadline; retry after it finishes`                     |

A permission problem is never reported as "another operation is running", and a
failure before any start command is never reported as "the Windows startup
command failed". Exit status remains `1`.

## Diagnostics

`env status`, the Settings diagnostics and the exported report include the
`coordination-lock` check. It opens and verifies the same private (and, where
applicable, legacy) maintenance files and immediately tries the shared flock.
Descriptors close before slow diagnostics, so the preflight never waits for UEFI
recovery or holds a VM reservation through Docker/HTTP probes. A failure
carries `coordination-lock-busy`, `coordination-lock-untrusted`, `coordination-lock-unavailable` or
`coordination-identity-mismatch`, is listed in `readiness.blockingChecks` and
clears `ready`, so the readiness flags match what operations will actually do.
This preflight is independent of startup attempts, so a lock failure that occurs
before a startup attempt is recorded is still visible.

## Verification

`winboat::maintenance::tests` covers `755`/`775`/`700` Compose directories with a
`664` Compose file, private `600`/`640` files, symlinked/hardlinked/FIFO/directory
entries, public or symlinked namespaces, exclusive vs shared exclusion across
different Compose paths of one VM, older versions holding the legacy lock
(shared, exclusive and Runtime stop), unsafe existing legacy locations that fail closed without being followed, identity mismatch, and distinct causes/messages. Real child
processes with separate config/cache/TMPDIR prove UEFI exclusion, SIGKILL release
and Runtime stop serialization across processes. `cli::runtime_stop_tests` runs
real CLI stops against both a legacy owner and a current owner and checks the
exact untrusted reason for the private lock.

## Acceptance audit after #226

Existing legacy entries are now **always** part of the bridge, even if the Compose
folder changed from 755 to 775 after an older process acquired its flock. A held
file descriptor remains locked after chmod. Only an absent legacy entry in a
normal 775 folder is omitted; an existing symlink, unsafe mode or unknown owner
fails closed. A dangling Compose symlink or permission error is not absence, and
an absent Compose file still reserves its VM management identity.

The maintenance preflight now performs an immediate shared try-flock and reports
exclusive maintenance contention. The independent `runtime-stop-lock` check only
clears `readiness.runtimeStop`; it does not disable Studio or Runtime start.
Common blockers clear both `runtimeStart` and `runtimeStop`. Diagnostics never
wait for another participant or reserve the VM while probing Docker/Guest health.

Contract **6.0.0** provides stable `coordination_busy`,
`coordination_untrusted`, `coordination_unavailable`,
`coordination_identity_mismatch` and `coordination_timeout` codes. Cancellation
continues to use `external_process_cancelled` from the command lifetime. Causes
are no longer inferred from translated messages. Settings save and initial setup
report `environment.configure`, not `runtime.start`. Existing v4/v5 Runtime and
artifact records remain readable; private shared-browser descriptors accept v5/v6
and still reject the older incompatible v4 shape. The diagnostic report is v3.

### Explicit in-app permission repair

For owner-verified nonstandard permissions, Settings offers **Review coordination
permissions**. The preview lists the exact private folder and regular files,
including the two existing legacy Mendimaru lock files for the selected VM.
Explicit confirmation is required. The single-use, five-minute preview is bound
to the selected VM, Compose path, directory/file inodes and reviewed modes.
Changed entries, unknown files, links, hardlinks, foreign owners and active locks
are refused before chmod. The repair uses held descriptors, sets only the private
folder to 700 and listed files to 600, preserves file contents/inodes and does not
recursively chmod, unlink files, change Compose parents or touch WinBoat storage.
If the app cannot safely verify an entry, it remains blocked; there is no terminal
command or root installer modification of a user's home as a fallback.
