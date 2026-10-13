# Keeper-owned Studio stop across startup window handoffs (#234)

## Cause and scope

Studio Pro 11.12.6 can replace its initial main window with its Sign In/main
window after the launch monitor has reported a visible window. The original
monitor consumed an authenticated stop request and called `CloseMainWindow()`
once. Delivery of `WM_CLOSE` does not establish process exit: the startup root
could consume it while a later root remained alive. The public keeper stop
then timed out after 45 seconds; an explicit second request closed the later
root. Live diagnosis showed different guest window handles for these requests,
both with successful message delivery.

The production helper `studio_stop.ps1` retains that exact owner's stop intent
for at most 30 monotonic seconds. It is embedded in the hash-authenticated launch
and reconnect scripts and also used by the standalone guest stop script. No
public contract fields, HMAC envelopes, keeper socket format or Runtime record
formats change.

## Required behavior and boundaries

| Boundary                                                        | Behavior                                                                                                                                   |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| No authenticated owner request                                  | No stop state and no window action.                                                                                                        |
| Forged, wrong-session, wrong-PID/start-time or replayed control | Existing HMAC/target/sequence validation rejects the request before it can initialize stop state.                                          |
| Accepted request                                                | Consume the control file, advance its sequence, then initialize bounded pending intent.                                                    |
| Missing or disabled main window                                 | Keep pending intent; do not claim the close was delivered.                                                                                 |
| Valid current root                                              | Request normal close only after refreshing and validating the exact PID/start-time.                                                        |
| Accepted `WM_CLOSE`                                             | Remember its window handle; do not infer process exit.                                                                                     |
| New root in the same exact process                              | Carry the original owner request through the startup handoff.                                                                              |
| Same/previously handled root                                    | No duplicate close; do not re-open a cancelled save dialog.                                                                                |
| Save or confirmation modal                                      | No button activation, key injection, force kill, or other process action. The application/user retains its decision.                       |
| Observation or close exception                                  | Not absence or delivery; remain bounded and fail closed.                                                                                   |
| PID/start-time mismatch                                         | Never act on the replacement process.                                                                                                      |
| Guest clock change                                              | Stopwatch bounds and 250 ms polling throttle, not guest wall clock.                                                                        |
| Deadline                                                        | Expire intent so a future window is not closed indefinitely after a timeout. An explicit owner retry creates a new intent.                 |
| Process exit                                                    | Only observed exit leads to the authenticated final empty-session report. Existing host report/absence validation remains unchanged.       |
| Runtime/Compose cleanup                                         | Existing serialized keeper cleanup and participant exclusion remain unchanged; no additional RDP client or Compose recreation for polling. |

## Verification

`scripts/test-studio-stop.ps1` runs the production helper against an in-memory
process stand-in. It checks idle, missing/disabled windows, delivery vs exit,
window handoffs, de-duplication, explicit retries, PID/start-time mismatch,
exceptions, throttle and expiry without touching a user desktop. CI runs it
under Windows PowerShell 5.1 alongside the existing guest helper tests.
Rust template tests verify helper embedding in launch/session scripts, exact
identity arguments and complete placeholder replacement.

The real Debian 13 / FreeRDP 3.15.0 / Studio Pro 11.12.6 reproduction confirmed
that the old first stop timed out and its explicit second stop succeeded. With
the helper, the first authenticated stop succeeded after a fresh Studio launch;
no user save/sign-in dialog was answered and no Studio process was force-killed.
