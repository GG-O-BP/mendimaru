# WinBoat lifecycle regression matrix

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
