# Connectivity, readiness and health

The GUI, safe CLI environment result and diagnostic report expose the same
independent fields. Existing `guestOnline` and `ready` remain compatible.

- `connectivity`: Guest API reachability on Linux; local workspace connectivity on
  native Windows. It does not authorize an operation.
- `readiness`: `studioLaunch`, `installation`, `uninstallation`, `projects`, and
  `runtimeStart`, `runtimeStop`, and `blockingChecks`. Platform support is also required. Missing or unknown status
  is never sufficient to enable an action.
- `health`: `attentionRequired` plus the stable IDs in `attentionChecks`. This
  includes warnings as well as failures, without examining translated text.

| Diagnostic                                                                                               | Launch / uninstall / projects | Installation          |
| -------------------------------------------------------------------------------------------------------- | ----------------------------- | --------------------- |
| WinBoat, Compose, coordination lock, runtime, FreeRDP, shared directory/mount, container, Guest API, RDP | Required success              | Required success      |
| Guest clock                                                                                              | Nonblocking attention         | Nonblocking attention |
| Marketplace browser                                                                                      | Nonblocking attention         | Required success      |

The coordination lock check verifies Mendimaru's private maintenance and Runtime
stop locks without holding them; see [coordination locks](winboat-coordination-locks.md).
`blockingChecks` lists failed or warning common prerequisites; browser availability
is an additional installation-specific prerequisite. Native Windows evaluates only
its applicable checks and retains its existing native connection label and controls.

On Linux, clock skew can therefore mean connected, ready and attention-required
simultaneously. RDP or mount failure can mean connected but not ready. Header and
Studio notice provide a text-labelled diagnostics action; color is supplementary.
Startup failure remains a distinct, sticky lifecycle state and takes precedence
over the connected presentation until a successful startup attempt.

`runtime-stop-lock` defects disable only `runtimeStop`; normal Studio readiness
and Runtime start remain available. Maintenance contention disables common work
with a distinct busy diagnosis.

The readiness flags are UI affordances, not a security boundary. Backend operations
continue to validate their own current process, runtime, mount and session
preconditions at execution time; a previously rendered snapshot is never authority
to bypass them.
