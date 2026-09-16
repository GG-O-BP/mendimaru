# Issue #21 reboot checkpoint — 2026-09-16, second handoff

## Resume here

The latest user instruction is to **save and push before a PC reboot**, then
continue in a new session. This checkpoint pauses implementation; it is not a
completed acceptance report. The earlier authorization remains: finish **Linux +
WinBoat**, create the PR and merge verified work into main. Native Windows
transport/Studio acceptance is deferred. Keep issue #21 and `needs-multi-os` open
for that remainder; do not use a closing keyword in the PR.

- Repository: `GG-O-BP/mendimaru`; issue: <https://github.com/GG-O-BP/mendimaru/issues/21>.
- Worktree: `/home/ggobp/Workspaces/mendix/mendimaru-issue-21`.
- Branch: `feat/21-winboat-uia`.
- Draft PR #167: <https://github.com/GG-O-BP/mendimaru/pull/167>.
- Original checkout `/home/ggobp/Workspaces/mendix/mendimaru` must be preserved.
- Latest implementation before this checkpoint: `580427f`; main `dea7c05` was
  already merged in `ede9cc7`. PR was MERGEABLE, still draft, at checkpoint.
- No applicable AGENTS.md or skill was found; no subagents were used. This is
  **mendimaru**, not pantosDemoAgGrid.

Read the current #21 body **and comments**, plus #16, #17, #6 and #24 before
resuming. Also read [provider documentation](winboat-ui-automation.md),
[the #16 spike](winboat-studio-uia-spike.md), [backend contract](backend-contract.md)
and [CLI contract](headless-cli.md). Issue #21 requires exact-session Ready → F5,
observed build/deploy/runtime phases, modal diagnostics and an actual Linux
Chrome app assertion **without request interception**. Those combined acceptance
gates are still incomplete. Hosted Windows CI is not native Studio acceptance.

## Reboot shutdown and private archive

Public `studio stop` successfully closed the current Studio 11 session. Its
Runtime automatically reached `stopped`; the original WinBoat Compose file was
restored **byte for byte**, and the keeper and RDP client exited. The owned assets
watcher, Xvfb `:121` and xfwm4 were stopped. WinBoat shutdown verification is
recorded in the archive's `shutdown.json`. The four other issue VMs had already
been stopped. Unrelated `kangs-paste-issue2-20260916` runtime/Postgres containers
were preserved.

Durable private archive (mode 0700, deliberately outside git):

`/home/ggobp/.local/state/mendimaru/issue-21/20260916T040952Z-reboot`

It contains the entire private lab as `lab/`, temporary helper scripts/logs as
`tmp-files/`, selected fixture model backups, test results and a SHA-256 manifest.
The older `20260916-handoff` archive remains unchanged. **Do not upload these
archives wholesale:** configs, Compose backups, screenshots and helper material
can contain credentials or user data. This document contains no credentials.

The old lab location was `/tmp/mendimaru21-resume`; private helpers hard-code it.
If it is gone after reboot, restore `lab/` there with mode 0700, then copy only
needed helpers from `tmp-files/` into `/tmp`. Do not treat restored session/cache
records as live. Freshly verify ownership, processes, VM disk, ports, certificate,
config, fixtures and exact Studio versions. Never reuse old PIDs, UI element IDs,
RDP handles or the old exclusive VM-use assumption. Do not replay historical
retirement/diagnostic commands.

## Saved implementation and unverified candidate

Contract version is 5.0.0. Unchanged v4 Runtime/build/browser records remain
readable. The public CLI uses the existing exact-session keeper and persistent
MTA worker, signed bounded mailboxes, deadline/cancel handling, 512 MiB child job,
one-use hash-pinned redirected bootstrap and private screenshot artifacts.
Adapters are pinned to Studio file versions **10.24.26.0 and 11.12.4.0**.
Unsupported actions fail explicitly; no arbitrary script/process or coordinate
input interface was added.

Recent commits:

- `e075b46`, `25421f4`, `e64f8c9`: UTF-8 pipe/BOM handling, navigation keys,
  safe error diagnostics and authenticated old-monitor retirement.
- `f96b0c9`: nested modal input ownership and Korean login handling.
- `dff9fe7`: preserve ownership on failed reconnect; fix PowerShell 5.1 parsing
  of multiple installed Studio records.
- `13bff5e`: cache per-node UIA observations; bounded unique native Go To search
  editor accepts the existing restricted identifier value. Native WPF tests
  cover unique/ambiguous/unrelated editors and stale handles.
- `580427f`: preserve disabled native owner roots but omit their blocking child
  trees, explicitly reporting truncation and `omittedDisabledWindows`. Global
  lookup/waits reject partial observations except a positive enabled-modal wait;
  scoped lookup in an enabled modal works. Native fixture tests passed.

**This checkpoint also saves a candidate in `ui_worker.ps1` and
`test-ui-navigation.ps1` that is NOT yet live-verified:**

1. Cache pattern-availability properties, avoiding per-node current
   `GetSupportedPatterns()` calls. Native fixture compares cached/live patterns.
2. Ask the validated native element to focus before foreground activation, while
   retaining exact owned foreground verification. Native fixture checks focus.
3. Exclude omitted disabled window roots from modal classification. Studio's
   Notification Stack can advertise UIA enabled while Win32 says disabled.

Both edited PowerShell files parse successfully; the eight Rust UI tests passed
at this checkpoint. Those checks do not execute Windows UIA or prove the new
candidate's behavior. The last live keeper and debug executable used **580427f**,
not these candidate edits. Rebuild and start a fresh keeper before testing them.
Do not describe this commit as a measured performance improvement yet.

## Validation already obtained

Before the latest candidate: Linux Rust 385 library tests, 8 contracts, 18 CLI
E2E and the lifecycle test passed; clippy passed. Frontend 111 tests in 23 files,
lint, production build, formatting and backend schema checks passed. Relevant
logs are archived (`m21-reconnect-all-tests.log`, `m21-reconnect-clippy.log`,
`m21-cache-ui-tests.log`, `m21-reboot-ui-tests.log`).

All CI jobs for **580427f** passed in run **35052853682**, including Windows
helper contracts, both host test suites, actual Windows Tauri dev E2E, installed
Windows bundles, security and AUR. Performance run **35052853526** was still
running: Windows WebView passed; Linux WebView and installed Windows bundle
measurements were pending. Check the newest checkpoint CI as well. Earlier
performance failures/superseded runs remain historical evidence, not waived gates.

Actual WinBoat tests passed authentication/expiry/wrong-target checks, helper
warm reuse/crash/restart/cancellation/release, BOM/Unicode and plural inventory,
nested modal ownership, and the bounded Go To/partial-owner native WPF fixtures.
These helper contracts supplement, but do not replace, actual Studio acceptance.

### Actual Studio 11: model edit and saved proof

The last session was `studio-5536-639251271135764846` (now stopped). Studio/helper
were both in interactive Session 2. Fresh launch took 145.258 seconds. Public
login-later, native tree and Ready worked. An intentional owned-RDP disconnect
caused `ui-session-unavailable`; public reconnect restored the same process/start
identity in 24.195 seconds.

The successful **public CLI only** edit sequence was:

1. File menu Ctrl+G → native Go To editor `Home_Web` → scoped Go To button Invoke.
2. Page opened in **Structure mode**, not Design mode. An earlier wait for the
   Design document timed out because of that incorrect harness expectation.
3. View → Properties, then View → Page Explorer. Semantic SelectionItem click on
   the observed Getting started widget selected it without coordinates.
4. Its editable Name property changed from `uia16Probe` to `uia21Probe11` through
   ValuePattern, followed by Tab and File/Ctrl+S.
5. A later independent `mxcli DESCRIBE PAGE` and the `.mxunit` contents confirmed
   the saved value. The first immediate read saw the old value because save was
   asynchronous; preserve that failure instead of claiming instantaneous save.
6. Public PrintWindow screenshot was reviewed and showed the selected widget
   and new Name. Page Name `Home_Web` itself was read-only and correctly rejected.

Evidence: `lab/fixture11-page-saved-confirmed.txt`, `fixture11-page-before.txt`,
`cached11-name-save-capture.json`, and `cache/ui-artifact-AXtXHC/window.png`.
Changed model unit: `mprcontents/7b/22/7b22940f-b2dd-43e1-9a0b-25011a7e1b33.mxunit`.

After switching back to App Explorer, public Ready wait passed and F5 dispatched
in 13.822 seconds. Trees observed the native Run Project progress dialog and
status texts including error checking, clearing deployment directory and writing
files. These were still classified as generic `modal`, not distinct build/deploy
states. **`cached11-run-phase-20.json` and `reboot-tree.json` finally reported
`running` with no dialogs.** The fresh reboot tree had 899 nodes and took 44.256
seconds. Runtime HTTP readiness and an actual Chrome assertion were **not tested**
before shutdown. A running UI observation alone does not prove app acceptance.

### Actual Studio 10: retained failure

Old session `studio-8436-639251251242346007` is stopped. Native trees, Ready,
menus, selection, screenshots and disconnect/reconnect passed. Public F5 reached
the actual build, but bundled Node/Rollup aborted with exit 134 at
`rollup-runner.mjs:65:13`; no working app was established. Full tree requests during
build/error modal timed out before the 580427f owner-omission fix. The underlying
bundler failure remains unresolved; see `build10-error-diagnostic.json`.
A positive Studio 10 Name edit/save is still missing.

## Fixtures, tools and continuation

Disposable fixtures are preserved on disk, not committed into this product repo:

- 11: `/home/ggobp/Workspaces/mendix/mendix-workspaces/UIA21_11_20260916/UIA16_10.mpr`,
  ID `project_49d11fcfc0a8f5c46825d6550f6bede4b6d4ea613218acd73f31384986773ff5`.
- 10: same parent, `UIA21_10_20260916/UIA16_10.mpr`,
  ID `project_bdaf39a3764a4bb6b00dbe7fa0a61df64b81473573cff64dbb5e9bf02d7f7186`.

Original #16 fixtures were preserved. Fixture 11 assets were hydrated, 39 widget
units and 19 design properties repaired with official exact 11.12.4 tooling,
and zero consistency errors verified before the UI edit. Fixture 10 had zero UI
consistency errors after hydration. Preserve the new saved Name and backups.

Private helper `m21lib.py` wraps the public CLI, saving JSONs in the lab. Full
trees/F5 need explicit `--timeout-ms 60000`; put `--timeout-seconds` after command
arguments. Its old `start11.json`/`start10.json` are evidence only after shutdown.
The shared debug target path is
`/home/ggobp/Workspaces/mendix/mendimaru/src-tauri/target/debug/mendimaru`.
Build from the issue worktree with that checkout's `src-tauri/target` as
`CARGO_TARGET_DIR` and `CARGO_BUILD_JOBS=2`.

Private guest test runners open another RDP connection and can disturb active
Studio UI tests. Run deliberately, serially. Historical `m21-guest-cache-diagnostic.py`
and `m21-guest-clean-error-short.py` target OLD PID 8436 and must not be replayed.
Never disable RDP TLS verification; discover current ports and use verified trust.
`chrome-assert.mjs` is prepared but **unrun**: real headed Google Chrome, sandbox
on, no interception, Home heading and `.mx-name-uia21Probe11`, no request/HTTP/
console/page errors. Revalidate URL/runtime/environment before using it.

Next work, after fresh environment verification:

1. Rebuild and validate this checkpoint's cached-pattern/focus/modal candidates
   against real guest fixture tests and Studio. Recheck CI/performance outcomes.
2. Fix conservative build/deploy/runtime phase classification using actual current
   progress context. In the Run Project dialog (`프로젝트 실행`), progress labels
   are depth 5 and current status text depth 8, with a duplicate child at depth 9.
   Do not infer current phase from static future/completed step labels. Keep
   unknown/blocking-modal behavior when evidence is insufficient.
3. Repeat Ready → F5 → HTTP-ready → real Linux Chrome assertion with the saved
   Name, no interception, and explicit phase/modal evidence. Investigate the
   Studio 10 bundler failure and obtain its positive edit/save proof.
4. Complete selected-process isolation using multiple actual Studios and final
   lifecycle cleanup evidence; helper wrong-target tests alone are insufficient.
5. Finish relevant tests, full current CI/performance gates and reproducible
   evidence/docs. Then mark PR ready and merge main as already authorized.
   Keep #21/native Windows remainder open. Do not request the same authorization
   again merely because this reboot checkpoint paused work.
