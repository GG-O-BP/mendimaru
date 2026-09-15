# Safe browser Runtime observation — issue #148

The browser metadata lookup now uses the registered Studio owner or bounded
keeper IPC. Missing metadata fails before browser execution with a retryable,
path-free diagnostic. Runtime observation, including readiness timeout diagnosis,
uses host/HTTP checks without opening another RDP connection.

RDP loss and observation errors preserve ownership, project access, and Runtime
forwarding. Automatic cleanup requires a newer authenticated Studio-exit report
or a successfully confirmed explicit Studio stop. Reports are bounded regular
files; authentication, sequence replay/reuse, failed-report, and file-read errors
cannot authorize cleanup. Unconfirmed keeper stops cannot open a fallback RDP
connection. Persisted schemas and public error enums are unchanged.

The earlier reboot checkpoint remains available at
[`db49275`](https://github.com/GG-O-BP/mendimaru/blob/db4927512552b2c8c5d359222fc1fabfd092ad0b/docs/issue-148-handoff.md).

## Automated validation

On Linux, after the final Rust changes:

- Ordinary Rust suite: **373 passed, 18 ignored** across library and integration
  targets. The ignored tests require separate live/environment gates.
- Frontend: **108 passed**; strict browser E2E: **13 scenarios passed**.
- Live-gate helpers: **2 passed**, including rejection of an unlinked Runtime
  and a replacement VM even when readiness remains successful.
- Rust Clippy, ESLint, formatting, frontend build, and backend contract validation
  passed. The schema remains `4.0.0` with 21 Linux WinBoat capabilities.

The ordinary suite exercises real keeper processes, Unix IPC, CLI dispatch, and
Chromium against isolated guest/Docker/RDP fixtures. It covers linked and local
owners, metadata failures, Runtime read commands and timeout, disconnection,
authenticated exit, explicit-stop serialization, and failed-launch cleanup.
These fixtures do not substitute for the actual VM evidence below.

## Actual Windows VM validation

The implementation at `80f4179` was tested on Linux/X11 using actual Studio Pro
**11.12.3**, an isolated copy of `IronCalcSpreadUIShowcase`, and Studio's Run
Locally action. The tested debug executable's SHA-256 is recorded in the
[safe evidence summary](issue-148-live-evidence.json).

The original VM was shut down cleanly. Its disk and UEFI state were copied into
snapshot `mendimaru-148-20260915-cold-reflink`, and the original/snapshot sparse
extent contents of all eight state files were compared. A separate working copy
was restored and booted with a healthy Guest API. The disposable VM used a
distinct container, storage, Compose, configuration/cache, and project copy.
An initial launch returned `operation_failed` before RDP launch; subsequent
installed-version discovery and Studio launch succeeded.

| Actual scenario                                      | Observed result                                                                                                                                                                          |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Runtime `/login.html`, strict console/network checks | Two browser runs passed; ten Runtime status reads passed.                                                                                                                                |
| Before/during/after those commands                   | All 29 samples preserved container ID, Compose SHA-256, every published port, Studio PID/start identity, keeper PID/start identity, and RDP PID/parent/start identity.                   |
| Temporarily unavailable keeper socket                | `browser.test` returned retryable `precondition_failed` in **0.275 s**, explicitly stating that no RDP connection was opened. All identities were preserved and the socket was restored. |
| Terminate only the actual FreeRDP client             | Twenty samples over ten seconds preserved the VM, Compose, ports, and Studio identity; the keeper reported `unknown/disconnected`.                                                       |
| Explicit Studio stop after that disconnection        | The retained authenticated control channel confirmed exit. The keeper exited, the Runtime became `stopped`, and Docker events showed exactly one Compose recreation.                     |

The successful browser interval was **2026-09-15 13:56:07.708–13:56:16.915 UTC**.
Studio PID was `8852`, keeper PID `25023`, and RDP PID `26120`.

### Separate app failure and observation limits

The first home-page browser run failed because `dist/widgets.css` returned 404
and the page imported `LanguageSelector.css` as a JavaScript module. All 15
lifecycle samples remained unchanged during that failed run. This is separate
from #148; the successful login-page gate establishes Runtime browser execution
and lifecycle preservation, not successful rendering of the demo's widget pages.
The failed browser artifacts were retained rather than treating that run as a
successful app test.

Studio process identity is supplied by the authenticated owner; this test does
not open an independent Windows process-query connection. Process sampling is
500 ms plus command duration and can miss shorter-lived processes. The ordinary
fixture's zero-RDP-launch assertions complement that sampled live evidence.

## Environment restoration and evidence

The disposable VM was stopped. The original VM was restarted with the **same
container ID and unchanged Compose bytes**, and its Guest API became healthy.
Docker reassigned its dynamic system ports within the original configured ranges;
the fixed Runtime port remained `8080`. No system-port settings were rewritten.

Private raw reports, browser artifacts, and the verified snapshot are retained
outside Git in the local `mendimaru-148-live` evidence directory. The checked-in
summary contains only safe observations and SHA-256 references, with no process
arguments, project paths, credentials, or Compose content. The reusable gate is
documented in the [regression matrix](winboat-regression-matrix.md#safe-browser-and-runtime-observation-148).
