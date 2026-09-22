# UI job coordination on the shared WinBoat helper (#152)

Multiple callers reuse one session keeper, its authenticated guest channel and
the persistent UIA helper from [#21](winboat-ui-automation.md). This document
defines how those callers are coordinated: who may overlap, in which order
same-session jobs run, who owns a job, and what cancellation or failure is
allowed to touch. The provider itself is not duplicated here; Windows native
coordination stays deferred with it.

Issue boundaries: [#148](issue-148-verification.md) fixed the read path so
observation never replaces RDP sessions, and [#150](winboat-vm-use.md) added
same-VM use leases. This layer composes both. [#149] is the parent parallel
execution effort and [#155] adds the shared-Runtime browser gate on top.

## Job classes

Every accepted UI request is classified before it touches the guest:

| Class       | Requests                                                           | Overlap rule                                        |
| ----------- | ------------------------------------------------------------------ | --------------------------------------------------- |
| Observation | `capabilities`, `tree`, `find`, `screenshot`, semantic `wait`      | Shared VM use; parallel candidate across sessions   |
| Session     | `action` with `invoke`/`click`/`set-value`, `release`, `reconnect` | Serial per session, in accept order                 |
| Desktop     | `action` with `focus` or `keyboard-input`                          | Serial per session and exclusive across the desktop |

The split follows the #21 provider semantics. `invoke`, `click` and `set-value`
act through window-targeted UIA patterns and do not steal desktop focus, so
they serialize per session only. `focus` and `keyboard-input` act on the
foreground of the single interactive desktop, so they exclude each other across
every session, window and cooperating process of the same VM — including two
separate keepers for two Studio windows. The exclusion is an advisory
same-UID flock (`/tmp/mendimaru-ui-desktop-<uid>/<key>.lock`), keyed by the
same runtime + management-name identity as VM use, in its own namespace, and
held only for the action's duration.

**Same-session capability boundary.** The guest bridge owns a single request
mailbox per session (`#21`), so exactly one in-flight guest request may exist
per Studio. Requests for one session — observation or mutation alike —
therefore serialize in keeper-accept order; overlapping them would corrupt the
authenticated mailbox, and the queue is what makes that impossible now that
the keeper accepts connections concurrently. Observation parallelism applies
where the transport supports it: across sessions (separate keepers, channels
and workers) and at the desktop scope.

VM use stays conservative: observations acquire a shared VM lease and may run
beside other shared participants in other sessions; every Session or Desktop
job acquires an exclusive VM lease as before, so no state change overlaps
another participant's use or a lifecycle transaction.

## Order and arrival

The keeper accepts connections concurrently and assigns each one an arrival
number at accept time. A same-session job's queue position is that arrival:
jobs for one Studio execute in keeper-accept order regardless of task
scheduling races between connections. An accepted-but-unparsed connection
reserves its position conservatively; if it turns out to be `status`, `stop`,
an invalid line, or never sends one, the reservation is dropped and later
arrivals proceed.

The queue grants one turn at a time per session; everything behind the head
waits in arrival order. A semantic `wait` holds its turn for its whole bounded
poll — that is the job boundary — so requests accepted after it queue behind
its deadline or fail bounded and retryable (`ui-coordination-busy`) within
their own budget.

`status` and `stop` do not join the UI queue: they are keeper administration,
not Studio UI jobs, and remain available while long observations or waits run.
`stop` waits up to three seconds for in-flight UI jobs to drain before the
keeper exits; every job keeps its own bounded deadline regardless.

## Caller identity and ownership

Each job records the caller's kernel peer credential identity (PID plus
`/proc` start ticks where readable) together with the exact Studio session it
addresses — the session ID already embeds the Studio PID and start ticks — and
the window the request named, when it named one. The identity labels ownership
only: authorization is the same-UID socket credential plus the existing
authenticated channel, and the label is never persisted or used to signal,
kill or retarget a process.

Cancellation — caller EOF on the keeper socket, or the request's own deadline —
removes only that caller's job:

- a queued job leaves the queue and is never executed;
- a running job is cancelled through its existing signed cancellation request;
- no other caller's queued or running job, dialog, project or session is
  cleaned up, and no turn is force-released on another's behalf.

If a caller vanishes between admission and execution, the granted turn is
released automatically when its final reference drops; a queued entry whose
admission future is dropped is skipped at the next grant. Neither case can
strand later arrivals.

## RDP loss versus Studio termination

A failed observation or action is a bounded job failure
(`ui-session-unavailable`, `ui-helper-exited`, `ui-modal-blocked`, …). The
coordination layer never reacts to it by reconnecting, retargeting, tearing
the VM down, or guessing coordinates or another session. Only the keeper's
monitor observation of the Studio process itself (`registered_session_ended`)
concludes that the session ended and triggers the existing cleanup path.
Reconnection stays an explicit `ui reconnect` request by a caller that owns
the session, with its own verification and retirement semantics from #21.

## Budgets and errors

One request's `timeout_ms` bounds each waiting phase: queue admission and the
desktop foreground scope together may consume at most one full budget, VM
lease acquisition is bounded by what remains, and the guest request keeps its
own envelope. The caller-visible reply deadline for a queued request is at
most `2 × timeout_ms + 6500 ms`.

New bounded reason: `ui-coordination-busy` (`precondition_failed`,
`retryable=true`) — another caller holds this session or desktop longer than
this request's own budget. Everything else uses the existing closed reason
set; `ui-cancelled` covers cancellation while queued or running.

## Observing during changes

Observations are point-in-time reads, and on one session they never interleave
with another in-flight request (see the capability boundary above): the tree
or capture you get reflects a moment when no other accepted request was
executing on that session. Element IDs are per-generation as in #21 — use IDs
from the latest observation and re-read after mutations instead of assuming
stability. The capability notes (`truncated`, `omittedDisabledWindows`,
effect-unverified errors) remain the authority on what an observation proved.

## Verification record

Fake queue tests (hermetic, no guest) — `cargo test --manifest-path
src-tauri/Cargo.toml --all-targets`:

- `ui_automation::coordination::tests::same_session_jobs_run_in_accept_order_even_when_tasks_race`
- `…::same_session_requests_serialize_while_other_sessions_admit`
- `…::cancelling_a_queued_job_removes_only_that_callers_job`
- `…::queue_wait_is_bounded_by_the_requests_own_deadline`
- `…::provider_failures_release_the_turn_and_keep_ownership_bounded`
- `…::dropped_reservations_and_other_sessions_never_block_admission`
- `…::foreground_jobs_exclude_each_other_across_sessions_and_processes`
- `…::separate_desktops_do_not_exclude_each_other`
- `…::history_stays_bounded_and_running_jobs_are_counted`
- `…::job_classes_follow_the_provider_action_semantics`
- `…::coordination_busy_is_a_retryable_precondition_failure`
- `winboat::vm_use::linux::tests::desktop_scope_excludes_other_opens_and_frees_when_released`
- `winboat::vm_use::linux::tests::desktop_scope_keys_one_namespace_per_vm_identity`
- `cli::ui::tests::serve_ui_never_strands_the_queue_after_rejected_requests`

Live Linux + WinBoat interactive verification (Studio 11.12.4, one keeper
session, two real CLI callers): see
[issue-152-live-evidence.json](issue-152-live-evidence.json), reproduced by
`python3 scripts/spikes/issue152-live-evidence.py <studioSessionId>`. The
recorded run shows: a `ui tree` accepted while a 12 s semantic `wait` held the
session turn executed after it (18.2 s wall = 12 s turn + tree), two
concurrent `ui tree` callers ran additively through the single mailbox
without corruption, `studio status` answered in 0.3 s while UI jobs ran,
killing a 45 s `wait` caller freed the queue immediately for the next
arrival, focus attempts stayed bounded (`ui-foreground-lost`), and the same
session (same PID, no reconnect, no teardown) served every later observation.
Caller-side wall times are not turn intervals; strict ordering and
desktop-scope exclusion are proven deterministically by the fake queue tests
above.
