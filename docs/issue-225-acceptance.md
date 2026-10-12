# #225 acceptance audit

The initial #226 merge resolved the observed installation precondition, but its
closure comment counted unexecuted initial-setup/Runtime checks as completed.
#225 was reopened to correct that evidence and cover the remaining edge cases.

## Required behaviors and results

- Normal user-owned Compose parents 700/755/775, including umask 0002 and a 664
  Compose file: same private VM-identity locks; no Compose-folder chmod.
- Different Compose/config/cache/TMPDIR paths, absent Compose and later creation:
  same management identity; no lock unlinking or split inode during migration.
- UEFI exclusive versus ordinary shared and Runtime-stop exclusive serialization:
  real child processes, legacy holders, mode changes during ownership, cancelled
  multi-file waiters and crash/release fixtures are covered.
- Private entries: exact 700/600, owner, regular type, nlink=1 and no-follow;
  existing unsafe entries fail closed, including dangling Compose links. Fresh
  files are normalized only through their newly created descriptors, never by
  repairing an existing path without confirmation.
- Busy/trust/I/O/identity/timeout have distinct typed public codes. Cancellation
  is propagated by the existing command lifetime. CLI redacts all free text and
  GUI/operation history retain localized causes. Settings uses its own capability.
- Independent maintenance try-flock and stop-only diagnostics cover early
  failures and capability-specific readiness without mislabeling startup.
- Nonstandard owner-verified modes have an explicit inode-bound in-app preview
  and repair; links, other owners, stale previews and busy files are refused.
- First setup and repeated settings save use the same path-based service as the
  GUI commands. Fixture tests cover success, pending retention on error and
  unchanged Compose bytes/modes without recreation.

## Live initial setup evidence (October 12, 2026)

The explicit `live_complete_pending_setup_without_vm_mutation` test passed
against the real pending configuration. Before invoking the production completion
service it verified the detected/preferred Compose and shared directory were the
same and that the mount preview required **no** recreation. The service persisted
`winboatSetupPending: false`; it then checked the persisted result and exact
Compose bytes and directory mode remained unchanged. No VM restart, recursive
permission operation, lock deletion or manual Compose-folder chmod was performed.

The prior installation evidence remains valid: Studio Pro 11.12.6 installed and
was detected on the same 775 host. Studio-start failure after forwarding
recreation remains #227, a distinct RDP readiness/pinning defect; it is not used
as successful Studio/Runtime acceptance evidence.

The fixture acceptance and live initial-setup evidence must be kept distinct.
Live uninstall or Runtime lifecycle mutation requires a disposable VM snapshot;
the user's installed Studio and data are not an acceptance fixture.
