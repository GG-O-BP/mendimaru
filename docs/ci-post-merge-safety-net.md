# Post-merge performance safety net

The [split measurement SLA](release-performance.md#split-measurement-sla) moved
the 300-second idle phase and the whole `installed-bundle` suite off the
pull-request path so that pull-request checks finish inside ten minutes. That
trade is only defensible if a regression found _after_ the merge cannot be
quietly ignored. This is the machinery that makes it hard to ignore.

## What happens on a post-merge failure

1. `release-webview-gate` and `installed-bundle-gate` each upload the candidate
   report they evaluated. The gate writes its own verdict into that report, so
   everything downstream quotes the gate rather than re-deriving a verdict from
   the budgets. Re-deriving would let the filed issue and the gate disagree the
   moment a budget changes.
2. `post-merge-regression-report` runs when a build, measurement, or gate job
   reports `failure` on `main` outside a pull request. This includes the case
   where a failed measurement causes its downstream gate to be skipped. It is
   the only job in either workflow with `issues: write`.
3. It files an issue labelled `ci:perf-regression`, carrying the failing
   commit, the baseline, the run URL, the failing jobs, and a table of the
   violated metrics. A `push` failure with a measured budget violation also
   receives `revert-candidate`.
4. `perf-regression-hold` in `ci.yml` fails on every pull request while an
   issue carrying **both** labels is open.

## Deliberate decisions

**Deduplication is keyed on the commit, not the title.** The issue body carries
a `<!-- post-merge-performance-regression:<sha> -->` marker. Re-running the same
commit comments on the existing issue instead of opening a second one, and the
marker keeps working after a human edits the title.

**A failure with no budget violation is not called a product regression.** It
may be infrastructure or an artifact error, but it may also be a product crash
before a report existed. The issue is still filed for triage, but the available
evidence cannot attribute it to the merge, so `revert-candidate` is withheld and
it does not hold unrelated merges. Labelling an unattributed failure as a
revert candidate would send a human to revert code without evidence.

**A scheduled failure is not attributed to one merge.** The weekly run measures
whatever is on `main`, so no single commit is implicated and `revert-candidate`
is withheld there too. It is recorded but does not hold unrelated merges.

**A manual run from a feature branch cannot file a main regression.** The
reporter is restricted to `refs/heads/main`; `workflow_dispatch` on another ref
may measure that ref, but it cannot put the protected branch on hold.

**The issue is filed even when the reports cannot be read.** A missing or
corrupt artifact degrades the issue to "detail unavailable, see the run"; it
never suppresses the issue. Losing the record is the exact failure this job
exists to prevent, so the artifact download is `continue-on-error`.

**Only a hard `failure` counts.** A `cancelled` or `skipped` gate measured
nothing. Treating those as failures would file an issue every time the
concurrency group supersedes a run.

**The hold fails closed.** An unreadable query result raises rather than
reading as "`main` is healthy". The hold is also deliberately ungated: a hold
that a path filter could skip is not a hold. It costs one API call, so it does
not meaningfully affect the pull-request wall clock the split SLA protects.

**A pull request can never hold merges**, even if it somehow carries the label,
because that would block the branch trying to fix the regression.

## Clearing a hold

Either fix the regression, or record why it is not one and then close the issue
or remove the `revert-candidate` label. Both are deliberate, attributable human
acts; there is no automatic expiry. The broader `ci:perf-regression` label can
remain when the investigation should stay visible without holding merges.

Note that `changeControl.performanceFailureRerun` stays
`preserve-original-failure`. Re-running a failed performance job to get green is
not a way to clear a hold.

## Decided: the hold is advisory, not blocking

`Post-merge performance hold` reports on every pull request. It is deliberately
**not** a required status check. The required checks on `main` are, and remain:

`Test (ubuntu-latest)`, `Test (windows-latest)`, `Dependency security audit`,
`Actual Windows Tauri dev E2E`,
`Build, install, launch, and uninstall Windows bundles`,
`Linux Tauri WebKit E2E`, `Rust tests and clippy (ubuntu)`.

So the hold turns the pull request red and stays visible in the checks list,
but it does not mechanically prevent a merge. A human decides whether the red
is worth stopping for.

### Why advisory won

A blocking hold has a cost that is easy to miss when it is designed and
impossible to miss when it fires: **one false positive stops every merge in the
repository**, including the merge that would fix it, and the only way out is a
human removing a label.

That is not hypothetical here. The first production firing of this safety net
was a false positive. The post-merge run on `504c28f` failed with **zero
measured budget violations**; the cause was a WebDriver 30-second async-script
timeout on the **baseline** leg, which structurally cannot be a regression in
the candidate. Two open issues describe the same shape of false positive in the
gates themselves — an idle CPU-percentage relative gate that fires on
percentage-point noise, and a latency relative gate decided by a single
nearest-rank outlier at n=3.

With a known non-zero false-positive rate and a blast radius of "all merges",
advisory is the honest setting. The signal is preserved in full; only the
automatic punishment is withheld.

### This narrows what the split SLA promised

State this plainly rather than let the documentation imply otherwise. The split
measurement SLA was accepted on the argument that a post-merge regression would
**block subsequent merges**. What is implemented, and what this section now
fixes as the final form, is weaker: it is **visible and red, but advisory**.

The gap is real, and it is a deliberate trade, not an oversight. The reasoning
is that an advisory hold that a human actually reads is worth more than a
blocking hold that gets routed around — and every route around a blocking hold
(removing the label, closing the issue, disabling the check) destroys the same
signal the hold exists to preserve, while also training people to clear holds
reflexively.

### Raising it to blocking later

Making the hold blocking is a repository-settings change, not a code change:
add `Post-merge performance hold` to the required status checks on `main`. No
file in this repository needs to change, and it is equally cheap to reverse.

The precondition is evidence, not preference. Raise it once the false-positive
rate is low enough that a stopped repository is a proportionate response —
concretely, once the relative-gate false positives tracked in the idle and
latency gate issues are resolved and a stretch of post-merge runs has fired the
hold only on genuine, attributable regressions.
