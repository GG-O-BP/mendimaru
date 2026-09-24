# First IPC observation (#207)

[Run 35814756882](https://github.com/GG-O-BP/mendimaru/actions/runs/35814756882)
failed in the Linux baseline's second cold startup, before the deliberately
slow timeout/recovery probe. The shell was ready, but `get_environment_status`
inside WebKit's `execute/async` reached its 30,000 ms script deadline. The
candidate then did not run because the baseline command failed. The original
failure record and partial counts remain evidence; zero budget violations did
not provide a latency verdict.

The Linux first IPC now dispatches the same command exactly once through a
synchronous script, stores its promise outcome in the page and observes it with
short synchronous requests. This removes the long-lived WebDriver callback
from this measurement. A backend that does not settle still fails at 30 seconds;
rejection, page replacement, transport failure and timeout never trigger a retry
or substitute a sample. Tokens isolate calls and cleanup rejects late results.
The original logs cannot establish whether the backend or the async callback
stalled, so this is a transport mitigation, not proof of that historical cause.

`sampling.firstIpcTransport` declares `sync-poll-v1` on Linux and `execute-async`
on Windows. A comparison cannot mix the contracts. First-IPC timing includes
sync dispatch, polling (25 ms cadence) and cleanup overhead on both variants.
Startup still ends at the ready shell. Other IPCs and the deliberate server
script timeout probe keep their existing transport and deadlines. Budgets and
sample counts do not change. Original failed runs are never rerun for a green
replacement; new CI runs verify the changed harness.
