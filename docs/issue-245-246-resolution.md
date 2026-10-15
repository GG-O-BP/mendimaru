# FreeRDP selection and persistent WinBoat clock recovery

Issues: [#245](https://github.com/GG-O-BP/mendimaru/issues/245),
[#246](https://github.com/GG-O-BP/mendimaru/issues/246).
Structured, allowlisted observations are in
[live evidence](issue-245-246-live-evidence.json).

## Findings and resulting behavior

Mendimaru 0.6.3 correctly rejected its configured unsupported Debian FreeRDP
3.15.0, but a working 3.32.2 executable was already installed elsewhere. The
saved path still selected the system client. Lowering the guard allowed a real
Studio Pro 11.12.6 process and Sign In window to start, yet the rendered surface
stayed at Windows Welcome. `/gfx` did not repair it. The default-options trial
still failed after clock recovery, with measured guest skew −1.536 seconds.
The actual running executable's SHA-256 matched the unchanged system client.

Using 3.32.2 with production arguments rendered Sign In and Select App and
accepted mouse/keyboard input. All trials ended through authenticated normal
stop. This establishes a client-dependent difference on this environment,
independent of the clock error. It does not isolate the exact upstream change
or experimentally establish 3.24 as the minimum. The support floor remains a
conservative policy; the diagnostic bypass used for testing is not shipped.

Automatic discovery now probes candidates and prefers a supported client even
when an older `xfreerdp3` appears before a supported `xfreerdp` on PATH. Saved
explicit paths are preserved. Open settings expands Advanced settings and
focuses the FreeRDP executable field. Recovery text and
[FreeRDP guidance](winboat-freerdp.md) explain selection, saved paths, and the
tested boundary. No APT installation was needed for the actual recovery.

The guest's seven-hour boot error came from a UTC-valued QEMU RTC and a Pacific
Windows guest interpreting that RTC as local time. The container was UTC even
though QEMU specified `base=localtime`. `RealTimeIsUniversal` was absent and
Windows Time had Manual startup. Explicitly setting the RTC interpretation to
UTC and enabling automatic Windows Time persisted across normal restart and
full Compose recreation. The first available Guest API responses measured
−1.308 and −0.995 seconds respectively, within the existing five-second pass
threshold. The Windows timezone and container timezone were preserved.

`env repair-clock --rtc-base utc|local` provides that explicit persistent
repair through the existing authenticated, hash-pinned RemoteApp transport and
exclusive VM coordination. It refuses active keepers/connected clients, and
the guest checks administrator privileges and absence of any Studio process
before writing. The required RTC basis prevents guessing from the literal
`localtime` option. The result distinguishes settings application from current
clock verification and required reboot. The production command returned
`clockVerified: true`, `rebootRequired: false`, no blocking diagnostics, and
Studio launch readiness. An active Studio trial correctly returned
`precondition_failed` before clock repair.

Normal diagnostics do not change the guest clock. A failing clock check now
opens WinBoat rather than offering redetection as a repair. The corrected
[clock guide](winboat-clock-sync.md) explains the RTC/container/guest pairing,
explicit repair, partial failures, reboot verification, and separate NTP
troubleshooting without recommending kernel clock-source changes as a remedy
for an unproven cause.

## Verification

Backend regressions cover supported-client selection and failed/malformed
probes, required RTC options, and authenticated script generation. The UI
regression covers expanding and focusing the executable field. The production
PowerShell helper fixture passed on actual Windows PowerShell 5.1.26100.7920;
it checks UTC/local values and refuses missing/invalid basis, non-admin tokens,
running Studio, and failed process observation before changing settings. CI and
Windows release checks run that helper fixture as well.

The Tauri/WebKit suite passed all 49 functional, security, and performance
assertions. Run process-preservation browser fixtures separately from live RDP
launch/stop trials: those fixtures intentionally detect changes in the host's
FreeRDP process set. Live evidence contains screenshot hashes and bounded
clock samples; the owned-window screenshots remain private and no user project
was opened or changed.
