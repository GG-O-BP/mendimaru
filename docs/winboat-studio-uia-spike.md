# Studio Pro UI automation spike — Linux + WinBoat (#16)

## Checkpoint: 2026-09-15, awaiting host reboot

This is an **incomplete investigation**, not a Go/Limited Go/No-Go result.
The user requested Linux + WinBoat only and explicitly deferred native Windows.
Do not close #16 or enable product UI capabilities based on this checkpoint.

- Issue: <https://github.com/GG-O-BP/mendimaru/issues/16>
- Start comment: <https://github.com/GG-O-BP/mendimaru/issues/16#issuecomment-5680500584>
- Branch: `spike/16-winboat-uia`
- Worktree: sibling `mendimaru-issue-16` of the main checkout.
- Base: `cdc0d77` (`origin/main` when the worktree was created).
- No PR or merge yet. User has authorized both after verified Linux work.
- No applicable `AGENTS.md` was found in this repository or its ancestor directories.

### Actual preflight findings

| Check                    | Observation                                            |
| ------------------------ | ------------------------------------------------------ |
| Linux running kernel     | `7.2.4-arch1-2`                                        |
| Installed kernel modules | Only `7.2.6-arch2-1`                                   |
| FreeRDP                  | `3.31.1 (63b948ca5c)`                                  |
| Linux display            | X11, `:0`                                              |
| Configured container     | `WinBoat`, Docker, `ghcr.io/dockur/windows:5.14`       |
| Initial container state  | Exited; no running Docker containers                   |
| `docker start WinBoat`   | Failed creating a veth pair: `operation not supported` |
| `modprobe -n -v veth`    | No module directory for the running kernel             |
| CLI `env status`         | Connectivity false; Studio/project readiness false     |
| Administrator access     | `sudo -n true` requires a password                     |
| User's recovery choice   | Reboot, then continue this task                        |

The matching old kernel package exists in the package cache, but no system
module restoration was attempted. Compose, guest disks, user projects, app
settings and existing Studio sessions were not modified. The start attempt
failed before guest boot. No UI scenario ran: **zero measured trials**. Do not
represent this infrastructure failure as a Studio UI No-Go or as 0/20 UI
successes.

## Work prepared, not yet validated in the Windows guest

`scripts/spikes/studio-uia/capture.ps1` supervises an isolated MTA PowerShell
worker with a wall-clock timeout. `capture-worker.ps1` checks Studio PID, UTC
start ticks, exact executable file version and matching nonzero interactive
session, then collects bounded raw UIA trees and PrintWindow PNGs for visible
windows belonging to that PID. Captures require visual review; PrintWindow
returning true does not prove that a WebView/canvas was rendered. These are
read-only PoC tools, not the #21 product provider. They have **not run on
Windows**. Review and test their behavior before using them for evidence.

`evidence.mjs` validates a ledger of two exact versions (one 10.x and one 11.x),
20 planned trials each, cold/warm counts and seven explicit flow steps. Its
synthetic Node tests are bookkeeping tests, not Studio acceptance evidence.
Blocked/unexecuted trials cannot become successful UI measurements. Private
artifact paths and an `effectVerified` flag are required for passing steps;
the validator cannot establish whether an artifact actually proves an effect.

No version-specific locators, property changes, F4 actions or Run Locally flows
have been implemented or measured yet. No target 10.x/11.x pair has been chosen.
Local read-only MPR metadata inspection found existing sample projects at
11.12.0, 11.12.2 and an 11.12.0 alpha; these are discovery hints only, not approved
or measured targets. Keep other repositories, especially pantosDemoAgGrid,
outside this spike. Use disposable project copies.

## Resume after reboot

1. Read this checkpoint, #16's full body/comments, repository instructions and
   `CONTRIBUTING.md`. Check branch/worktree status and `origin/main` drift.
2. Check `uname -r`, the matching `/usr/lib/modules` directory, `docker ps -a`,
   FreeRDP and `mendimaru env status`. Start only the configured WinBoat guest.
   Verify guest API and RDP readiness before calling anything a live test.
3. Discover installed Studio versions and choose exact supported 10.x/11.x
   projects. Make disposable copies, record source hashes and avoid conversion
   of originals. Fix English UI, theme, 1280×800 resolution and 100% scaling;
   record actual guest OS/build, DPI, screen and executable versions.
4. Run Studio and the helper in the same interactive RDP session. Verify
   PID/start ticks/window ownership. Test failure paths: stale PID/start time,
   wrong session, missing window, hung provider, ambiguous/missing locator,
   modal dialog, read-only property, lost foreground and cleanup.
5. Collect App Explorer, Page Editor, Design/Structure mode, Properties,
   consistency errors and modal UI trees/screenshots. Inspect Properties and
   Preview separately. Compare .NET UIA with a pinned UIA3-based candidate and
   `winapp ui` against the same windows.
6. Author measured locators. Repeat the same representative flow **20 times
   per exact version**, including cold process launches and warm runs. Record
   page open, widget selection, property change with readback, Design and
   Structure modes, F4 synchronization with verified effect, and Run Locally
   with verified readiness. Keep every failed and incomplete attempt. Do not
   treat input delivery/exit zero as the intended app effect.
7. Compare Windows window captures with Linux RemoteApp captures, including
   canvas/WebView contents, occlusion and minimized windows. Explicitly test
   screen lock, RDP disconnect/reconnect and changed DPI. Reacquire elements
   after reconnect; record failures without extrapolating to native Windows.
8. Publish a version/locator/action compatibility table, reviewed and sanitized
   example artifacts, measured rates/failure reasons, session/window/dialog
   state model, minimum helper command contract and a justified decision.
   Decide explicitly whether coordinates can be a default (current proposal:
   no; this is not yet an experimentally justified decision).
9. Test/lint/format changed tools, review the diff, create a PR, wait for relevant
   checks, merge into main and update #16. Preserve native Windows as deferred
   follow-up scope; do not claim it was measured or silently close its scope.

## Candidate references checked during preparation

- [Microsoft UI Automation CLI](https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/ui-automation): pattern actions and input injection have different desktop requirements; capture output needs review in this application.
- [FlaUI](https://github.com/FlaUI/FlaUI): .NET UIA2/UIA3 candidate to compare, not a measured Studio result.
- [UIA threading](https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/ui-automation-threading-issues): keep provider calls off UI threads; a separate process supplies the hard timeout.
- [Mendix keyboard shortcuts](https://docs.mendix.com/refguide/keyboard-shortcuts/): confirm exact-version shortcuts and effects before replaying them.
- [Mendix Preview APIs](https://docs.mendix.com/apidocs-mxsdk/apidocs/pluggable-widgets-studio-apis/): these configure widget preview appearance; they are not evidence for external GUI control.

The inspected `winappCli` release is `v0.6.0`; x64 ZIP SHA-256 is
`f6dc42e3b4e4709c8f617003008e2cfdd9a51735e04e7170d60edda258db78a8`
from GitHub release metadata. It has not been installed or executed. Use
version-pinned documentation and verify downloaded assets before the comparison.

## Checkpoint validation

- Five synthetic ledger/classification Node tests passed.
- Changed JavaScript passed ESLint with zero warnings and Prettier formatting.
- Both PowerShell files parsed with PowerShell 7.6.6 on Linux; this does **not**
  validate Windows PowerShell 5.1, Windows APIs, ACLs, UIA or capture behavior.
- The parser download was verified against its GitHub release SHA-256
  `ddbc4a2d113bbd46d283cfedcbcd117a70caefd7673f41f2b4e0000badf103bc`.
- Full application tests were not run: no product code or capability changed.
- Before live use, review the prepared collector's output ACL/ancestor paths,
  locked/secure desktop identification, partial/truncated output, exact-version
  comparison and timeout cleanup. Keep it experimental until those cases pass.
