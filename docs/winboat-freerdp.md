# FreeRDP selection and Studio Pro rendering

Mendimaru's Linux support policy currently accepts FreeRDP 3.24+ within major
version 3. This is a conservative support floor, not an experimentally proven
claim that 3.24 is the first version that works or that every older distribution
build fails. The policy is shared by diagnostics, launch, reconnect, and automatic
discovery. Stop and cleanup stay available for an unsupported configured client.

In issue #245, the installed Debian 13 package
`3.15.0+dfsg-2.1+deb13u3` reproduced a stale Windows Welcome surface with Studio
Pro 11.12.6. Windows reported a real Sign In window, the X11 window title changed
to Sign In, and the authenticated launch report succeeded, while the visible
pixels remained at Welcome. Adding `/gfx` did not resolve it. Process existence,
window existence, and a successful CLI launch are therefore insufficient
evidence of a usable RemoteApp.

The default-options 3.15.0 trial was repeated after persistent clock recovery.
Guest HTTP clock skew was −1.536 seconds (passed), yet the Sign In window still
contained Welcome pixels after Tab. `/proc` confirmed the running executable
was the unchanged system client. This separates the rendering failure from the
seven-hour clock error. That diagnostic trial bypassed only the version
preflight; the installed executable and RDP arguments were unchanged.

Changing only the FreeRDP executable to the already-installed upstream 3.32.2
rendered Sign In correctly. Clicking Sign In Later opened Select App; Tab focus
rendered in its search field. No account or project was changed. These trials
establish a working recovery on this VM and a client-dependent difference. They
do not isolate the exact upstream patch, all possible flags, or all 3.15 builds.
Debian backports selected changes without changing the upstream version number.

Upstream 3.24 includes [RAIL window-locking changes](https://github.com/FreeRDP/FreeRDP/pull/12392)
and [an X11 timer fix](https://github.com/FreeRDP/FreeRDP/pull/12423). Those links
explain the support policy, not proof that either is this VM's specific cause.

## Select the installed supported client

1. Check the exact executable, rather than an unrelated shell alias:
   `"/absolute/path/to/xfreerdp" /version`.
2. Open Settings → Advanced settings → FreeRDP executable. Select the supported
   executable and save. The diagnostic's Open settings action opens this field.
3. Rerun diagnostics and launch Studio. Check actual rendering, mouse input,
   keyboard focus, and normal stop. The 3.32.2 client is verified on this VM.

Automatic discovery now probes both `xfreerdp3` and `xfreerdp` candidates on PATH
and prefers the first supported client. An older system `xfreerdp3` no longer
hides a supported `xfreerdp` later in PATH. Discovery is bounded; if no supported
candidate is found it retains the first candidate so diagnostics can explain the
failure. Explicitly saved paths are preserved; installing another client does
not silently replace a user's configured executable. Redetect the field or
choose its absolute path, then save.

Debian 13's configured APT repositories offered the same 3.15.0 package during
this investigation. Reinstalling that candidate would not change this result.
There was no need to install packages because a working 3.32.2 build already
existed locally. Other environments should use their distribution's supported,
patched client where available; this investigation does not prescribe mixing
distribution repositories or disabling certificate validation.

## Cold connection recovery (#257)

A later cold-session investigation reproduced a FreeRDP 3.32.2 abort in
`xfAppWindowsLockFrom`: the HiDef RemoteApp repaint path acquired an already-held
RAIL window lock. The Studio process survived the lost transport. Querying with
ordinary RemoteApp windows (`/app:...,hidef:off`) returned its exact live identity
and distinguished an older stopped identity. Mendimaru pairs that option with `/network:lan` and
`/tune:FreeRDP_SupportGraphicsPipeline:FALSE` for authenticated RemoteApp operations.
The explicit network mode prevents FreeRDP from re-enabling GFX through its
automatic connection defaults after argument parsing. HiDef off with GFX still
enabled avoids the abort but leaves X11 window pixmaps unpainted; transport and
guest UIA success alone are therefore insufficient validation.

Studio start initializes its monitor before the guest success report and then
requires an authenticated UI capabilities response before the host reports
success. UI reconnect also verifies that channel within its request budget.
A failed readiness check is a non-retryable start failure; it does not replay the
launch. The keeper and Runtime link remain available for supported status,
reconnect and stop, and unverified identities must be inspected with `studio status --refresh` before another start.

Public `studio stop` can retire an unreachable keeper after authoritative
absence of its exact Windows PID/start identity. Guest query failure or a live
identity preserves ownership. Older keepers without the retirement protocol are
retired only through a same-user IPC peer pinned with a Linux pidfd, after its
session identity is verified; PID reuse and a wrong-session reply cannot select
another process. No guest force-kill or direct lock deletion is required.
