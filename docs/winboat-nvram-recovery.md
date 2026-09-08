# Confirmed WinBoat UEFI recovery

This optional recovery is deliberately narrow: a QEMU boot timeout from the
current startup attempt **and** an entirely zeroed or erased (`0xff`) 540,672-byte
`windows.vars` file are required. A timeout alone, a normal-looking variable
store, another file size, or another boot mode never exposes the recovery button.
This does not claim to diagnose every possible NVRAM corruption.

Supported storage is one absolute, direct, user-owned Docker bind mount at
`/storage`, belonging to the uniquely identified WinBoat Compose service. Named
volumes, Podman, nested/shared/ambiguous storage, symlinked ancestors, symlinks,
hardlinks, writable-by-others directories/files, custom entrypoints, and boot
modes other than `windows` are rejected. The runtime service, image, immutable
container ID, mount identity, Compose digest, file inode, size, and contents are
rechecked against a five-minute, single-use preview.

The filename and regeneration behavior follow the upstream
[QEMU boot script](https://github.com/qemus/qemu/blob/4c20fb80a6116dd0348bfcc79cf018bf4d0cd401/src/boot.sh):
the `windows` boot mode uses `windows.vars`, and an absent variable file is
recreated from the firmware template. Mendimaru never enables the upstream
`CLEAR` operation, which also removes firmware ROM and TPM state.

The Settings button opens an exact preview with the target, backup, retained
original, byte count, and SHA-256. Explicit confirmation is required. The existing
file is copied to a collision-free timestamped backup, synced, and then moved to
a separate retained-original name without overwriting anything. Windows disks,
Compose, ROM, TPM state, and installed apps are not edited or deleted.

Mendimaru holds a cross-process maintenance lease through restart and Guest
readiness verification. Studio/install/session actions and Runtime start/stop
cannot overlap recovery. The container must be stopped before changing UEFI
files. If readiness fails, Mendimaru stops that exact container, revalidates the
preconditions, preserves any failed regenerated variable store, and restores the
original inode. If safe restoration is unavailable, the backup and original stay
intact and Settings offers a separately confirmed Restore UEFI backup action.
That action requires Windows to be stopped and the original identities unchanged.

Recovery is not guaranteed. The in-app pending preview/restore token belongs to
the current app process; after an app/host interruption, the timestamped backup
and retained original remain on disk for recovery through WinBoat while Windows
is stopped. Keep the exact paths from the preview. The feature never guesses
which backup to restore and never overwrites an unexpected target.

Ordinary CI uses temporary fixture directories only. A real recovery experiment
requires both explicit mutation opt-in and a disposable VM/storage snapshot;
the user's normal WinBoat storage is never an E2E fixture. Booting a real Windows
VM naturally writes to its own disk; the byte-for-byte invariance tests assert
that Mendimaru's UEFI file transaction does not touch the fixture disk or Compose.
