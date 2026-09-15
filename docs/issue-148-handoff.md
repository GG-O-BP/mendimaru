# Issue #148 reboot checkpoint — 2026-09-15

## Status and authorization

Work is **in progress**, not ready to merge. The user requested a reboot
checkpoint and push while implementation/testing was underway. No PR has been
created, main has not been modified, and #148 remains open.

The original task authorizes an issue-start comment, a new branch and worktree,
implementation, verification, PR creation, and merge into main. Resume that
task after reboot; no renewed permission for the PR or merge is needed.

- Repository: <https://github.com/GG-O-BP/mendimaru>
- Issue: <https://github.com/GG-O-BP/mendimaru/issues/148>
- Start comment: <https://github.com/GG-O-BP/mendimaru/issues/148#issuecomment-5680497878>
- Branch: `fix/issue-148-safe-browser-runtime`
- Worktree: `/home/ggobp/Workspaces/mendix/mendimaru-issue-148`
- Original worktree: `/home/ggobp/Workspaces/mendix/mendimaru` (main)
- Starting main commit: `cdc0d77f6743cee8af94f42a6580ebc6be4d9aed`
  (`fix(cli): preflight keeper sockets before launching Studio (#158)`)

No applicable AGENTS.md was found in the repository or its parent chain.
`CONTRIBUTING.md` and `docs/winboat-regression-matrix.md` were read. The normal
Rust suite is required for these keeper/lifecycle changes. No subagents were
used. Other worktrees and unrelated repositories were left alone.

## Implemented so far

- `application::browser_test_runtime` obtains Studio version from the local
  registered owner or bounded keeper IPC via `winboat::observed_session`.
  It does not fall back to a Windows/RDP session query. Missing, timed-out,
  malformed, wrong-session, wrong-schema, invalid-version, and stopped metadata
  produce a retryable `precondition_failed` for `browser.test` with an exact,
  path-free diagnostic preserved by CLI sanitization.
- Registered sessions survive RDP client exit and report-read/authentication
  failures. Disconnected sessions retain their identity, control state, and
  project lease, with process state `unknown`. Only an authenticated newer
  successful report with no sessions marks Studio stopped. Stopped records are
  hidden from ordinary registered listings but retained until keeper cleanup
  (or the next registration).
- The keeper cleans linked runtimes only after confirmed Studio exit or a
  successful explicit stop. Socket observation failures do not trigger teardown.
  Its observation timer now runs independently of frequent IPC requests.
- Failed keeper stop confirmation returns an error rather than allowing the CLI
  to open a second RDP connection. A failed registered stop preserves ownership
  even when its RDP client exited.
- Launch/reconnect PowerShell monitoring no longer turns arbitrary observation
  exceptions into a successful Studio-closed report. This still needs actual
  Windows validation.
- A further read-path audit found the terminal `runtime wait` diagnostic still
  opened RDP. That probe and its obsolete tests were removed. Timeout diagnosis
  now uses host/HTTP observations; linked Studio state uses the same safe owner
  lookup and missing metadata means `unknown`, not `stopped`. Persisted schemas
  and public error enums are unchanged. Documentation for the former listener/
  firewall diagnostics has **not yet been updated**.

## Regression tests and verification

`src-tauri/src/cli/runtime_stop_tests.rs` uses isolated subprocesses running real
CLI dispatch and the actual keeper loop/Unix socket. Docker, guest HTTP, and the
RDP client are fixtures. The browser cases run the real Playwright runner and
installed Chromium.

Added cases cover keeper-linked browser testing, the GUI/local-owner path,
metadata failure boundaries, RDP disconnection with missing/tampered reports,
later authenticated exit, runtime read commands, and terminal readiness timeout
with both present and missing owner metadata. #146's Compose race fixture now
provides an authenticated Studio-exit report: RDP loss alone must not initiate
automatic cleanup.

Verification completed before the final read-path changes:

- `cargo test --manifest-path src-tauri/Cargo.toml --lib cli::runtime_stop_tests
-- --nocapture --test-threads=1`: **10 passed, 0 failed**, 19.61 seconds.
  This included real keeper IPC and Chromium browser execution.
- The earlier #146/#147 fixture-only run passed all 6 tests.

The current checkpoint includes changes made **after** that 10-test pass:
the shared safe observation helper, independent keeper timer, removal of the
terminal Runtime RDP probe, and additional runtime-read/timeout assertions.
Do not claim the 10-test result validates those later changes.

The full `cargo test --all-targets -- --nocapture --test-threads=1` build was
started but deliberately terminated for the user's reboot checkpoint (exit 143),
before test results. It reported a dead-code warning caused by an accidentally
removed `#[cfg(test)]` on `security_probe_script`; that annotation was restored
before saving. Rebuild and verify the current checkpoint from scratch.

`cargo fmt --check` and `git diff --check` are the final checkpoint checks.
Local raw logs are preserved outside Git in
`/home/ggobp/Workspaces/mendix/mendimaru-issue-148-checkpoint/`:
`targeted-tests.log` and `interrupted-all-tests.log`.

## Resume

1. Read this handoff, the current #148 body/comments, `CONTRIBUTING.md`, and
   `docs/winboat-regression-matrix.md`. Inspect the worktree and remote branch;
   fetch main and account for any intervening merges without discarding work.
2. Restore frontend dependencies. This worktree temporarily used a symlink to
   `../mendimaru/node_modules`; the checkpoint removes that untracked symlink.
   Recreate it if still appropriate, or run `npm ci`.
3. Complete code review, especially preservation/cleanup of disconnected owner
   records, failed stop handling, authenticated terminal reports, and the
   compatibility implications of `observe_studio` during launch reuse. Audit
   remaining read paths for unexpected RDP. Explicit Studio discovery still has
   its original query behavior; do not silently return incomplete discovery as
   authoritative absence or weaken #99's cleanup safety.
4. Run the targeted regression tests and the entire ordinary Rust suite. The
   previous invocations reused the original worktree's build directory:

   ```bash
   CARGO_TARGET_DIR=/home/ggobp/Workspaces/mendix/mendimaru/src-tauri/target \
     cargo test --manifest-path src-tauri/Cargo.toml --all-targets -- \
       --nocapture --test-threads=1
   ```

   Serialize use of that shared Cargo directory. Then run Rust clippy, formatting,
   contract validation, and relevant browser checks. Fix any regression rather
   than relaxing existing security/compatibility assertions.

5. Add the **still missing live regression gate** for an actual keeper-linked
   Studio F5 runtime: compare container ID, Compose hash, published ports,
   Studio PID/start identity, and RDP processes before/during/after browser test.
   Include metadata diagnostics and absence of teardown. At inspection time
   `docker ps` showed no running containers, so no actual Windows VM/F5
   reproduction has been performed. Do not describe fixtures as live VM proof.
   Follow the repository's disposable-VM rules for any mutating live lifecycle
   test and accurately report unavailable verification.
6. Update `docs/browser-testing.md`, `docs/winboat-run-locally.md`,
   `docs/headless-cli.md`, and the regression matrix as appropriate. In
   particular, old docs currently promise an RDP listener/firewall diagnostic
   that the checkpoint removed. Record the correction to the old assumption
   that external concurrent work necessarily caused VM recreation.
7. Once reviewed and verified, create the requested PR with a clear final
   problem/behavior summary, test evidence and live-test limitations. Merge into
   main and close #148 only when the work is ready. Remove or update this
   checkpoint document so unfinished-state claims do not remain misleading.
