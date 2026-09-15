# Issue #21 reboot handoff — 2026-09-16

## Request and current status

The user requested issue #21 implementation, a starting issue comment, a new
branch/worktree, a PR, and a verified merge into main. The authorized scope is
**Linux + WinBoat only**; native Windows UI transport and acceptance are deferred.
The user then requested saving/pushing this checkpoint before reboot and resuming
in a new session. Work is **incomplete**. Do not merge, close #21, or describe the
Linux acceptance matrix as passed yet.

- Repository: `GG-O-BP/mendimaru`.
- Worktree: `/home/ggobp/Workspaces/mendix/mendimaru-issue-21`.
- Branch: `feat/21-winboat-uia`.
- Original checkout: `/home/ggobp/Workspaces/mendix/mendimaru` (preserve it).
- [Draft PR #167](https://github.com/GG-O-BP/mendimaru/pull/167).
- [Starting issue comment](https://github.com/GG-O-BP/mendimaru/issues/21#issuecomment-5688224321).
- Initial base: `0ec892c` (#165, issue #16 spike).
- Implementation commits before the reboot checkpoint: `08ed37f`, `ac6f566`.
- Latest fetched main: `dea7c05` (#166, browser environment-change reporting).
  GitHub reports the PR as **DIRTY**. Resolve those conflicts before expecting new
  pull-request CI checks. Do not overwrite #166's changes when aligning schemas.

The user confirmed other work had used/recreated the VM, then granted this task
exclusive VM use. After reboot, freshly check its ownership and environment; do
not assume old ports, PIDs, RDP sessions, or the old exclusive-use window survive.
No applicable AGENTS.md was found during this work. No subagents were used. This
is **mendimaru**, not pantosDemoAgGrid; the latter's product contract is unrelated.

## Read before continuing

Read current issue #21 body **and comments**, plus #16, #17, #6 and #24. Their
bodies/comments were read before implementation. Also read:

- [UI provider documentation](winboat-ui-automation.md).
- [Issue #16 spike](winboat-studio-uia-spike.md): Limited Go, bounded helper only;
  no generic canvas automation or assumed full frozen-flow completion.
- [Backend contract](backend-contract.md) and [CLI contract](headless-cli.md).

Issue #21's additional acceptance comment requires exact-session project Ready,
Run Locally/F5, distinct build/deploy/runtime observations, modal diagnostics,
and an actual Linux Chrome app assertion **without request interception**.
Those positive end-to-end gates have **not passed** at this checkpoint.
Native Windows deferral must remain explicit in the eventual PR/issue outcome;
retain `needs-multi-os` tracking and do not auto-close #21 with a closing keyword.

## Implementation map

- `src-tauri/src/ui_automation/mod.rs`: bounded typed requests, validation,
  operation/error mapping, Linux dispatch and explicit reconnect work in progress.
- `src-tauri/src/ui_automation/bridge.rs`: signed request/report/cancellation
  mailboxes over the registered Studio channel, freshness checks, bounded private
  screenshot artifacts and Linux file-safety checks.
- `src-tauri/src/cli/ui.rs`: CLI parsing, values through stdin, same-UID private
  keeper IPC, serialization and cancellation acknowledgement under a VM lease.
- `src-tauri/scripts/ui_supervisor.ps1`: persistent MTA child, private NTFS source,
  job-owned cleanup and 512 MiB child limit, signed deadline/cancel supervision.
- `src-tauri/scripts/ui_worker.ps1`: exact PID/start/path/version/session guards,
  UIA tree/find/wait, semantic actions, conservative state/dialog classification,
  PrintWindow capture. Adapters currently declare **10.24.26.0 and 11.12.4.0**.
- Launch/reconnect scripts embed the supervisor and hash-pinned worker. Existing
  session keeper and `UiAutomationBackend` contracts are reused.
- `winboat/security.rs` and `remote_app.rs`: operation keys moved out of guest
  process arguments into a private, one-use RDP redirected bootstrap file. The
  guest argument pins its SHA-256. File removal precedes execution; directory
  lifetime belongs to the retained RDP child. This affects existing operations
  too, so retain their regression tests.
- Contract version is **5.0.0**: the closed CLI enum/result contract changed.
  Unchanged v4 Runtime/build/browser records remain readable. Active schemas,
  runners and fixtures were updated; historical evidence was not rewritten.
- `scripts/test-ui-helper.ps1`: production worker/supervisor failure/lifecycle
  test using a non-Studio target. It is **not native Studio acceptance**.
- CI now has a separate, short Windows helper-contract job, with test-only
  initialization stderr capture to diagnose a hosted-runner failure.

## Verified results and practical limits

### Linux regression results

Before the final unverified Unicode/navigation edits:

- Full Rust all-target tests passed. Library result: **378 passed, 18 ignored**;
  binary/integration test executables also passed.
- Clippy all-targets with `-D warnings` passed.
- Backend JSON contract validation passed (v5, 21 capability IDs).
- Frontend: 111 tests / 23 files passed; production build and frontend lint passed.
- Prettier and Rust formatting passed.

At the reboot checkpoint, PowerShell syntax parsing and `cargo check
--all-targets` passed again. This is **not** a replacement for rerunning the full
relevant tests after the last edits and conflict resolution.

### Actual WinBoat observations

A dedicated Xvfb `:121`, xfwm4, isolated config/cache and real FreeRDP client were
used. No UI command opened a new RDP connection. Actual Studio 11.12.4 was started
through the public CLI with a disposable project ID.

- Production helper initialization, warm reuse, wrong-process rejection,
  crash/restart, release, signed cancellation, expired request, authentication
  rejection and UTF-8 script BOM passed in the real guest contract test.
- Studio and UI helper both ran in interactive **Session 2**, not Session 0.
- Tree and PrintWindow screenshot worked. The login capture was visually checked.
- Korean login-later button was invoked through its observed UIA ID.
- Login window classification was fixed: this window is **not** reported modal
  by WindowPattern, but its title must still produce a `login` dialog diagnostic.
- `ui wait --condition project-ready` succeeded on the loaded app.
- Exact `MyFirstModule` lookup succeeded; `click` selected its SelectionItem.
- `InvokePattern` on that App Explorer DataItem failed. Do not claim it opens a
  document. Its child expander has **TogglePattern**, not InvokePattern.
- Tree scans during project transition sometimes returned `ui-provider-failed`;
  a later stable observation passed. Transient-error handling still needs review.
- Foreground requests sometimes failed safely with `ui-foreground-lost` while
  the notification window/RemoteApp focus changed. Later trees showed the main
  Studio foreground. UIA invoke/select/value operations no longer unnecessarily
  require keyboard foreground; focus/keyboard still do.
- The new private redirected bootstrap successfully started a second real
  Studio session, and its one-use file was consumed.

Old session IDs, **evidence only; both were explicitly stopped**:

- `studio-8328-639251067080306226` (first implementation).
- `studio-9008-639251075408124531` (private-bootstrap implementation).

Both public `studio stop` commands returned success. The task's Xvfb and test RDP
clients were stopped. Do not reuse these PIDs or session IDs after reboot.

### CI failure still requiring investigation

For `08ed37f`, the hosted Windows job passed Rust tests and both clippy modes,
then failed the new helper test with `ui-helper-exited` during initialization.
The same helper test passed in WinBoat. Do not classify this as native Studio
acceptance failure or waive it merely because native transport is deferred.

- CI run: `35030081701`, Windows job: `104586276147`.
- Raw job logs may require `gh api .../actions/jobs/ID/logs
--allow-escape-sequences`; strip terminal escapes before presenting logs.
- `ac6f566` adds a dedicated short helper job and test-only bounded stderr capture.
  At checkpoint, no new run existed because the PR conflicts with main.
- Old `08ed37f` CI and Release performance runs may finish after this checkpoint;
  inspect their results, but they do not validate the latest commit.

## Latest saved changes that are not yet live-verified

1. **Unicode pipe encoding:** Windows PowerShell 5.1 needed a BOM when the worker
   source was written to disk; that fix is live-verified. A second issue was
   found: ASCII `MyFirstModule` lookup works but a Korean File-menu lookup
   returned no matches. The parent `Process.StandardInput` code page is suspect.
   The latest supervisor sends explicit UTF-8 bytes through `BaseStream`.
   Add a real Unicode pipe regression test and verify exact Korean lookup/value
   input. Do not count the earlier BOM test as verifying this new fix.
2. **Semantic navigation:** latest `click` also supports TogglePattern with state
   readback. Keyboard whitelist adds Enter, Right and Escape, alongside Tab,
   F5, Ctrl+G and Ctrl+S. These need live tests and documentation reconciliation.
   No coordinate input or arbitrary script/process action was added.
3. **Explicit `ui reconnect`:** keeper-side reconnect is present but not tested.
   It reuses the backend's exact-session reconnect and project-access rules.
   Review cancellation during reconnect, old helper/monitor cleanup, retained
   protected-project constraints, and bounded timeout behavior. A live keeper
   running older code cannot serve this new operation: start a fresh verified
   keeper for the recovery test.
4. **Error diagnostics:** worker emits only CLR type/HRESULT, never exception
   source/message text. Bridge constructs a `uia:TYPE:CODE` diagnostic reference,
   but CLI sanitization currently accepts artifact references only, so it likely
   drops this value. Finish a safe diagnostic representation and regression test.
5. Invoke dispatch returns a pre-action element snapshot to avoid reporting a
   false failure solely because a successful invoke removed the element. Verify
   this behavior; invocation dispatch still does not prove application success.
6. Build/deploy/starting-runtime states use supported shallow WPF text and retain
   observed text. Running requires the enabled Stop button under the Console
   tab. These phase distinctions have not been observed through a complete F5 run.

## Disposable projects and private evidence

Persisted **local private archive**, excluded from git:

`/home/ggobp/.local/state/mendimaru/issue-21/20260916-handoff`

It contains CLI JSONs, screenshots, Linux test logs, guest helper-test results,
private harness scripts, and the isolated config/cache. **Do not upload it wholesale**:
Runtime recovery copies may contain original Compose credentials. Old `env.json`
points into `/tmp`; recreate display/authentication and refresh config paths.

Shared disposable fixtures:

- `/home/ggobp/Workspaces/mendix/mendix-workspaces/UIA21_11_20260916`
- `/home/ggobp/Workspaces/mendix/mendix-workspaces/UIA21_10_20260916`
- Helper-test-only files: `.mendimaru-ui21-helper-tests` under the same share.

Both projects are copies; the issue #16 originals were preserved. Their MPR
basename is `UIA16_10.mpr` (retain it with the v2 mprcontents structure). Metadata
was checked read-only in `_MetaData`:

- v10: `10.24.26.123458` (installed file version `10.24.26.0`).
- v11: `11.12.4` (installed file version `11.12.4.0`).

Opaque project IDs in the original shared-root configuration:

- v11: `project_49d11fcfc0a8f5c46825d6550f6bede4b6d4ea613218acd73f31384986773ff5`
- v10: `project_bdaf39a3764a4bb6b00dbe7fa0a61df64b81473573cff64dbb5e9bf02d7f7186`

The initial minimal copies lacked template assets; the v11 UI showed **835 model
errors**. Before pause, only the disposable v11 project's javasource,
javascriptsource, theme, themesource and widgets were hydrated from the archived
`source11` template. This has **not been verified to repair that fixture**. Verify
model consistency and dependencies before attributing F5 failures to the provider.
No actual Name-editor value change or complete runtime/browser assertion passed.

Installed paths last observed (re-discover after reboot):

- `C:\Program Files\Mendix\11.12.4\modeler\studiopro.exe`
- `C:\Program Files\Mendix\10.24.26.123458\modeler\studiopro.exe`

VM is `WinBoat`; Compose is `/home/ggobp/.winboat/docker-compose.yml`. Runtime
forwarding setup/cleanup legitimately recreates the container and changes its
API/RDP ports. Read current Docker port mappings rather than using archived
4728x/4730x values. Preserve storage, guest preferences and other users' files.

Existing test helpers in the private archive are starting points, not commands
to replay automatically. `/tmp/m21-cli.py` used the shared cargo target executable
and isolated environment; `/tmp/m21-step.py` saved one JSON response and timing.
The private guest helper runner hash-verified copies before invoking the test,
kept credentials out of host argv, and used test-only diagnostic instrumentation.
Review it before reuse. Never print container credentials or encoded auth payloads.

## Suggested continuation order

1. Check worktree/main status, read this handoff and current issue contracts.
   Resolve latest-main conflicts carefully; keep #166 environment-change behavior.
2. Finish/review Unicode, navigation, reconnect and diagnostic changes. Add the
   targeted regression tests; reconcile help/docs/schemas with the final surface.
3. Run formatting, full relevant Rust tests, clippy, contract validation and
   frontend checks affected by conflict resolution. Diagnose the short Windows
   helper CI initialization error with its new test-only diagnostics.
4. Freshly verify VM identity, ports, RDP ownership and exact Studio installations.
   Recreate a private display/config/cache. A known trusted RDP certificate pin
   existed in the user's FreeRDP cache; verify identity instead of disabling TLS.
5. Verify disposable fixture consistency, then actual CLI UI tests on both
   declared adapters: semantic lookup/wait/click/value/capture, modal handling,
   stale/ambiguous/unsupported cases, multiple-process isolation, cancellation,
   helper crash, RDP disconnect/reconnect and Studio exit/ownership cleanup.
6. Complete exact-session Ready → F5/Run Locally → observed phases → real Linux
   browser assertion without interception. Capture diagnostic tree/screenshots
   for failures and report actual browser identity. Locally found Chromium
   caches are not automatically evidence of Google Chrome acceptance.
7. Write concise reproducible verification evidence, update the PR description
   around the final implementation, finish required CI, mark ready and merge.
   Update #21 with Linux completion and explicit native Windows deferral. Preserve
   unfinished multi-OS tracking. The user already authorized PR/merge; do not ask
   permission again because this checkpoint paused work.
