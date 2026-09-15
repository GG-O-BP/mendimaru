# Studio UI spike tools

These are experimental Windows-guest tools for the disposable #16 fixture.
They do not register product capabilities or implement the #21 provider. See
[the report](../../../docs/winboat-studio-uia-spike.md) and its
[frozen protocol](../../../docs/winboat-studio-uia/protocol.json).

## Reproduction layout

Use a private local NTFS lab directory and a separate private export directory.
The measured harness expects this layout:

```text
lab/
  studio10.json
  studio11.json
  project10/UIA16_10.mpr
  project10/mprcontents/...
  project10/widgets/StarRating.mpk
  project10/widgets/com.mendix.widget.web.Datagrid.mpk
  project11/... (the same basename in a separately converted project)
  winapp/winapp.exe
  capture.ps1
  capture-worker.ps1
  run-trial.ps1
```

Create a local Blank Web App with Studio 10.24.26.0, rename Home_Web's body
DynamicText widget to `uia16Probe`, and save. Close Studio before cloning the
source into `project11`, excluding deployment/build caches and locks. Preserve
both the MPR basename and `mprcontents`. Convert only that clone in 11.12.4.0.
Verify source hashes and a saved model description. The original measured
fixtures and private captures are retained outside the repository.

Each identity JSON must describe the exact currently bound Studio process:
`pid`, `startTicks` (UTC ticks as a string), `version` (executable file version),
`session`, `exe` (absolute executable path), and `project` (absolute fixture MPR
path). Cold trials close that exact old process if still present and launch the
recorded executable/project. The private lab must be writable only by its owner
and SYSTEM. Do not use identity files or fixtures from an unrelated project.

The helper must run in the **same active interactive Windows session** as Studio,
with the Default input desktop available. Set Studio to English (United States)
and Light, and the session to 1280×800 at 100%. The trial checks screen/system DPI
and the page document's theme/language URL, maximizes Studio, rejects stale
process identity, and uses freshly resolved UIA bounds for clicks. It has
fixture-specific locators and explicit failure paths; it is not a general UI
scripting API.

Download the exact candidate files in `candidates.json` to a private host cache
and verify before staging. Extract the pinned winapp executable into `winapp/`.
FlaUI comparison scripts used the .NET Framework 4.8 assemblies of Core/UIA3
5.0.0 and their pinned interop dependency; FlaUI is not required by `run-trial`.
Keep the respective license files with the extracted candidates.

```bash
node scripts/spikes/studio-uia/verify-candidates.mjs /private/candidate-cache
node --test scripts/spikes/studio-uia/*.node-test.mjs
```

## Capture

Invoke `capture.ps1` with the exact identity and a **new local NTFS output
folder**. It rejects reused output, UNC and linked output ancestors, applies a
private ACL and supervises an isolated MTA worker. The default worker limits are
48 levels, 3,000 nodes per window and a 30-second collection budget; the outer
timeout defaults to 45 seconds. A partial tree is a reported failure with retained
evidence. Review the PNG pixels even when capture APIs returned success.

## Representative trial

Run with Windows PowerShell 5.1 in an isolated **MTA child process under an
external 600-second process-tree supervisor**. The trial script alone cannot
interrupt a blocked UIA COM call. Keep input mutations serialized and retain
stdout, stderr, timeout and process-exit results. Use a new campaign identifier
for a genuinely separate campaign; never overwrite or relabel a failed trial.

```powershell
.\run-trial.ps1 -LabRoot 'C:\private\uia-lab' `
  -LabShare 'C:\private\uia-export' -Major 11 -Trial 1 `
  -Phase cold -CampaignId example
```

Trials 1, 6, 11 and 16 are cold; the others are warm. `flow.json` records measured
step effects and durations; `preflight.json`, UI trees and `cleanup.json` preserve
the supporting observations. Cold launch evidence is exported separately.
F4 temporarily moves an unused MPK and restores identical bytes; Run Locally
starts and then stops only the fixture's runtime. After any timeout, verify
package restoration and process ownership before proceeding. The outer
supervisor may have interrupted a PowerShell `finally` block.

The measured script intentionally remains frozen, including its observed
readiness and selection/property-state limitations. In the Studio 10 campaign,
the preview could outline a text widget while Properties still described the
page; the verifier correctly rejected the mismatched Name. A missing locator fails the current flow; input delivery
does not become an effect check. Inspect later state, preserve the failed trial,
and document any recovery before continuing. Do not silently add retries or
change the frozen script during the measured series.

## Evidence ledger

`evidence.mjs` validates exactly two targets and 20 planned trials each. Passing
steps need artifacts and an effect assertion; unexecuted steps remain unexecuted.
This validates bookkeeping, not the truth of an assertion. Keep raw evidence
private, review exported examples, and retain source hashes. A no-data campaign
has a null success rate, not a measured 0% UI success rate.
