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

## Open policy decision: is the hold blocking?

`Post-merge performance hold` reports on every pull request, but the required
checks on `main` are:

`Test (ubuntu-latest)`, `Test (windows-latest)`, `Dependency security audit`,
`Actual Windows Tauri dev E2E`,
`Build, install, launch, and uninstall Windows bundles`,
`Linux Tauri WebKit E2E`, `Rust tests and clippy (ubuntu)`.

Until `Post-merge performance hold` is added to that list in branch protection,
it is **advisory**: it turns the pull request red and is visible in the checks
list, but it does not mechanically prevent a merge. Making it blocking is a
repository-settings change, not a code change, and is left as an explicit
decision because it lets one unresolved performance issue stop all merges.
