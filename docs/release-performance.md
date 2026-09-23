# Release performance contract

Mendimaru keeps functional development E2E and release performance as separate
signals. The existing Linux debug/Vite and Windows `tauri dev` reports remain
fast functional gates. The `Release performance` workflow measures optimized
artifacts without including compilation or a development server:

| Platform | Suite              | Artifact                                                | Coverage                                                             |
| -------- | ------------------ | ------------------------------------------------------- | -------------------------------------------------------------------- |
| Linux    | `release-webview`  | AppImage                                                | Full WebView, IPC, fixture, navigation, and resource metrics         |
| Windows  | `release-webview`  | release executable with the test-only WebDriver feature | The same full metric meanings through WebView2                       |
| Windows  | `installed-bundle` | installed MSI and NSIS                                  | Cold/warm native-window startup and installed process-tree resources |

The WebDriver feature and fixture hooks are compiled only with Cargo's `e2e`
feature. Normal release artifacts do not contain the embedded driver or the
loopback Marketplace override. The separately installed MSI and NSIS checks use
ordinary release bundles.

## Measurement relevance on pull requests

On pull requests the workflow first asks whether the commit can change the
measured artifacts or their measurement contract at all. Product sources,
Tauri sources and packaged resources, build inputs, `scripts/perf`, the
installed-bundle smoke harness, performance policies and schemas, and the
workflow itself trigger the full suite. Unrelated automation such as
`scripts/aur`, functional-test fixtures, documentation, lint configuration, and
other workflows use the fast skip path. The classifier is shared by every
build, measurement, and gate job and is covered by unit tests, including all
non-Node resources declared in `tauri.conf.json`.

Skipped build and measurement matrix legs still publish their diagnostic
`skip-reason.txt` artifacts, while required gate jobs remain green without
installing dependencies or downloading those artifacts. Pushes to `main`,
scheduled runs, and manual dispatch always measure in full. A pull request that
touches a measured input gets the same matrix dimensions, sample counts, and
gates as before for every phase it still runs; which phases those are is
defined by the split SLA below.

## Split measurement SLA

Pull-request checks and exhaustive performance measurement have separate
wall-clock targets:

| Scope                                                          | Target     |
| -------------------------------------------------------------- | ---------- |
| Pull-request required checks, first start to last completion   | 10 minutes |
| Push to `main` and the weekly schedule, exhaustive performance | 15 minutes |

Pull requests keep the candidate and baseline binary builds, the fingerprint
cache, the full latency phase, and both the absolute and the relative gates,
plus every functional, security, and packaging check.

The 300-second idle window moves to push-to-`main`, the weekly schedule, and
manual dispatch. It was the single largest item on the pull-request critical
path: in run 35726346591 every idle leg measured 363 to 411 seconds while
latency measured 132 to 149.

Nothing about the measurement itself changes. The idle window is still 300
seconds, the sample counts are unchanged, and `performance/budgets.idle.json`
keeps the same thresholds, which are calibrated to a five-minute window and are
`CODEOWNERS`-reviewed with `environmentOverridesAllowed: false`. Shortening the
window instead would have required rescaling `privateMemoryGrowthBytes`,
`workingSetGrowthBytes`, and `processCountGrowth`, re-collecting baselines
across Linux and Windows for the executable, MSI, and NSIS package kinds, and
versioning the sampling contract. Only the point at which a leak regression is
detected moves, not the sensitivity with which it is detected.

The whole `installed-bundle` suite defers rather than splitting, because
`scripts/e2e/windows-bundle-smoke.ps1 -Performance` performs its seven
install/launch/uninstall samples and its 300-second idle window in one
inseparable pass. Packaging coverage on pull requests is unaffected: the
`windows-bundle` job in `ci.yml` still builds, installs, launches, and
uninstalls both the MSI and the NSIS installer on every pull request, using the
same script without `-Performance`.

The cost of this split is real and deliberate: an idle or leak regression no
longer blocks the pull request that introduced it and is instead detected on
`main`. A post-merge idle failure must therefore be treated as a revert
candidate, and `changeControl.performanceFailureRerun` stays
`preserve-original-failure` so the first failing result cannot be papered over
by a re-run.

That discipline is mechanised, but deliberately stops short of enforcement. A
failing post-merge gate files a labelled, deduplicated issue, and an open issue
with both the `ci:perf-regression` and `revert-candidate` labels turns the
`Post-merge performance hold` check red on subsequent pull requests.

That check is **advisory**: it is not a required status check, so it makes the
regression visible and red but does not mechanically block the merge. This is a
decided trade, not a gap waiting to be closed — the first production firing of
the safety net was a false positive, and a blocking hold would have stopped
every merge in the repository over it. Raising it to blocking is a
branch-protection setting rather than a code change. See
[ci-post-merge-safety-net.md](ci-post-merge-safety-net.md).

Both measured binaries are cached on a fingerprint of the inputs that can
change them, not on a commit sha. The fingerprint covers `src`, `src-tauri`,
`public`, `.cargo`, the frontend build inputs and every non-Node Tauri
resource — including the bundled `scripts/browser-*.mjs`, which live outside
`src-tauri` — together with the platform build recipe and the resolved `rustc`
identity, runner OS and architecture, and the native linker/compiler/SDK
identity. The broad hosted-runner image release is not used because GitHub can
roll two image revisions across parallel matrix jobs even when their
artifact-affecting toolchains are unchanged. A Tauri resource outside the
repository fails the fingerprint step rather than being silently omitted.
Each variant reads the resource declaration from its own measured revision,
not from the candidate checkout, so changing or removing an external resource
cannot make the baseline key omit a file that the baseline still packages.
Budgets, schemas, `scripts/perf` and the workflow itself are deliberately
excluded: they change what is measured, not what is built, and the relevance
classifier above already forces the full suite to run for them.

Caching both variants, rather than only the baseline, is what keeps the
comparison honest. A sha-keyed baseline could restore a binary produced by an
older toolchain while the candidate always compiled with the current one, so a
toolchain delta could be read as a code regression. Equal keys mean the two
variants now always hit together or rebuild together. When a revision changes
no build input the two fingerprints coincide and both variants measure the
same artifact, which is the correct reading of "nothing was rebuilt": the run
then reports run-to-run noise against the same absolute rails.

The build recipe lives in one place per platform and is fed both to the build
command and to the fingerprint salt, so changed build flags cannot silently
reuse a binary produced by the old ones. Release builds keep an ordinary
dependency cache for the genuine rebuild path; workspace-crate caching is
deliberately not enabled, because the repository's Actions cache is already at
its 10 GB ceiling and those entries would evict the far smaller binary caches
that remove much more critical-path work per byte.

That dependency cache stays keyed per variant on purpose. Both variants
compile the same crate graph, but they compile it in different directories,
and an Actions cache entry restores to the path it was saved from. One shared
key per platform would let whichever variant finished first publish a target
directory the other cannot use, converting its next restore into a full
dependency rebuild.

Those builds still refresh the crates.io index, measured at 31.4 s on the
Windows WebView leg and 4.0 s on Linux in run 35718372051. Skipping it with
`CARGO_NET_OFFLINE` on an exact dependency-cache hit looks safe and is not:
run 35720574481 failed the Linux baseline build with `no matching package
named aho-corasick found`. The dependency cache prunes the registry index
before saving, keeping the `.crate` files but not the index metadata that
cargo needs to rebuild its resolve graph, so an offline build cannot resolve
even though every crate it would download is already present. A developer
machine keeps a complete index and so cannot reproduce this; only the pruned
CI registry shows it. The refresh stays.

Baseline and candidate measurements run as parallel matrix jobs and a separate
gate job compares the two reports, so the previous strictly sequential
baseline-then-candidate schedule no longer doubles the wall time. On the
push-to-`main` and scheduled runs that carry the two 300-second idle windows,
this parallelism is what keeps the exhaustive run inside its own budget.

## Fixtures and measurements

Every full release-WebView run excludes one warm-up launch and records seven
samples for each latency metric. It uses a private safety-marked temporary root,
a loopback Marketplace server, and platform-specific environment fixtures. The
small workspace contains one project and about 1.1 KiB. The large workspace
contains 250 projects and about 62.6 MiB, so a one-project fast path cannot mask
filesystem-scan regressions.

The common metric meanings are:

- `coldStartupMs`: native process launch through a ready application shell after
  deleting the isolated WebView cache and user-data directory.
- `warmStartupMs`: the next launch with those WebView caches retained.
- `firstIpcMs`: the first normal `get_environment_status` IPC after each cold
  shell launch.
- `environmentSlowMs`: an environment IPC with the tracked 400 ms test-only
  backend delay. The loop discards one warm-up probe in the same slow mode
  before its measured samples, because it is the first IPC after the final
  launch and would otherwise fold the one-time cost of opening the IPC path on
  a fresh session into its first sample.
- `environmentTimeoutRecoveryMs`: time from a tracked 750 ms client deadline
  against a two-second delayed probe until the next normal environment IPC
  succeeds. The application is not restarted.
- `catalogCachedMs`: a disk-backed catalog read without starting a browser.
- `catalogRefreshMs`: a refresh from the isolated loopback Marketplace through
  a real sandboxed Chrome or Edge browser.
- `smallWorkspaceScanMs` and `largeWorkspaceScanMs`: repeated project discovery
  for the declared project counts and total byte sizes.
- `navigationMs`: WebDriver interaction through the Projects, Settings, and
  Studio Pro routes, ending when the target heading is visible. Sample index
  selects the route, so the three samples are three different workloads rather
  than three draws of one.
- `backgroundPollingCpuPercent`: the first twelve five-second CPU samples from
  the long-idle window, kept separate to expose periodic polling.
- `idleCpuPercent`, `privateMemoryBytes`, `workingSetBytes`, and `processCount`:
  sixty five-second samples covering at least 300 seconds.
- `privateMemoryGrowthBytes`, `workingSetGrowthBytes`, and
  `processCountGrowth`: positive end-minus-start leak signals from that same
  window. Negative deltas are retained in `resources.delta` but become zero for
  the upper-bound leak metric.

Idle samples use fixed five-second deadlines. Snapshot collection time is part
of each CPU interval and is subtracted from the next sleep instead of being
added after every interval. This retains sixty observations covering at least
300 seconds while avoiding an extra minute of PowerShell process-enumeration
overhead on hosted Windows.

Linux runs Xvfb inside a private D-Bus session because WebKitGTK desktop
services expect a session bus even on a hosted headless runner. Launch-stage
timing keeps WebKitWebDriver `POST /session`, the explicit ready-shell wait, and
process discovery separately attributable. The ready-shell assertion still
defines the end of every startup sample; no WebDriver response alone is treated
as application readiness.

Windows MSI and NSIS runs use the same cold/warm definition, repeat count, idle
window, process-tree scope, and resource meanings. Application data and WebView2
data are isolated and cleared by the guarded ephemeral-VM script. Installation
and uninstallation durations are reported for diagnosis but are not currently
regression-gated.

CPU is calculated for the root application and every live descendant visible at
each sample:

```text
(after process-tree CPU seconds - before process-tree CPU seconds)
----------------------------------------------------------------- × 100
             wall seconds × logical CPU cores
```

Private memory and working set/RSS are recorded separately. Reports contain the
start, finish, and peak process-tree snapshots so a stable endpoint cannot hide
a transient peak. CPU samples bind each PID to its process creation identity and
accumulate positive per-process deltas, so a short-lived child cannot make the
tree's CPU counter move backward. Linux additionally reaps adopted, exited
browser helpers; a growing zombie tree is still counted and fails the process
leak budget.

## Statistics and noise policy

The report retains every non-negative finite raw sample. It records nearest-rank
p50 and p95, minimum, maximum, median absolute deviation, IQR, and Tukey-IQR
outlier indices. Outliers are diagnostic only and are never removed from the
report or the selected gate statistic. Policies may gate p50, p95, or maximum;
the isolated catalog refresh uses p50 because three same-host runs showed a
stable 785–813 ms median while one-time browser bootstrap moved seven-sample
p95 between 874 and 1,728 ms. Windows warm startup likewise gates p50 after
three same-host comparisons kept candidate medians within 16 percent while one
984.97 ms Tukey outlier alone moved p95 by 28.62 percent. Both p95 values remain
visible in reports. At least five samples are required by the schema; the
tracked policy uses seven.

A metric may also split the two gates apart with `relativeStatistic`, which
selects the statistic for the relative comparison while `statistic` keeps
feeding the absolute ceiling. It defaults to `statistic`, so a metric that does
not declare it is unchanged. The Linux `environmentTimeoutRecoveryMs` gate
compares p50 and rails on p95, because its p95 is not a distribution tail: the
latency phase takes three samples after one warm-up, and across twenty-six
Linux sample sets the second sample was the maximum in twenty-two of them. A
nearest-rank p95 at n=3 therefore re-measures one reproducible blip whose size
swings between 1.05 and 2.36 times the median, so the relative gate was
comparing two draws of that blip rather than two tails. Three of thirteen
product-unchanged runs failed that way, including a +91.46 percent reading
whose candidate median was faster than the baseline median. Comparing medians
failed one of thirteen, and the 6000 ms absolute rail still reads p95, where
the largest observed value was 3165.51 ms. Reports print both statistics on a
split row so the comparison cannot be misread.

The Linux `navigationMs` gate splits the same way for a different reason. Its
three samples are not three draws of one workload: the harness walks the
Projects, Settings, and Studio Pro routes once each, so sample index selects
the route. Across thirty-eight Linux sample sets the per-index means were
100.8, 145.7, and 42.0 ms and the maximum fell on Settings in thirty-five of
them, which makes nearest-rank p95 at n=3 a single sample of the most
expensive route rather than a tail. The one relative failure in nineteen
product-unchanged runs read 116.84/132.87/59.27 against 76.46/266.63/67.72:
the median improved from 116.84 ms to 76.46 ms and a lone 266.63 ms Settings
sample failed the gate. Comparing medians failed none of the nineteen. What
this gives up is stated rather than hidden: relative coverage was one route
before the change and is one route after it, but it moves from the slowest
route to the middle one, so a regression confined to Settings now reaches only
the 1500 ms rail. Sample counts and absolute rails are unchanged.

The Linux `largeWorkspaceScanMs` gate splits for a third reason, and the
difference matters because it decides which remedy works. Its blip is not
positional. Across thirty-two Linux sample sets - sixteen runs, baseline and
candidate - the maximum fell on index 0, 1, and 2 in five, sixteen, and eleven
of them, so nothing about warm-up or route ordering explains it. What the data
does show is that a single sample out of three occasionally reads three to
four times the median: p50 stayed between 18.0 and 44.4 ms while p95 ranged
from 21.0 to 143.0 ms, and the largest candidate set was 26.9/34.2/143.0 ms
with a 7.25 ms median absolute deviation against a 116.08 ms IQR. At n=3
nearest-rank p95 is the maximum, so the relative gate was subtracting two
independent draws of that blip and read anywhere from -57.89 to +107.75
percent on runs that changed no product code. One of sixteen failed that way,
on a pull request that touched documentation, npm scripts, and a CI script.
Comparing medians failed none of the sixteen, and the candidate-to-baseline
p50 gap never exceeded 14.2 ms against the existing 50 ms floor, so a
sustained shift of roughly 35 ms is still caught. The 12000 ms rail still
reads p95, which is what bounds the linear scan regression this fixture
exists to expose; the largest value ever measured on it is 143 ms.
`smallWorkspaceScanMs` is deliberately left alone - its p95 and p50 differ by
at most 1.6 ms across the same thirty-two sets - and so is Windows, where no
such failure has been observed.

`catalogRefreshMs` uses metric-specific additive floors for a fourth reason:
the browser-refresh noise is measured in milliseconds, but a percentage-only
allowance shrinks whenever the baseline happens to be low. Seventeen
product-unchanged comparisons put Linux p50 between 706 and 1331 ms and the
same-run absolute p50 delta between 29 and 362 ms. The one failure was a
226 ms delta against a low 944 ms baseline, where 20 percent allowed only
189 ms. Linux therefore uses a reviewed 400 ms floor. This is paired with a
quality increase rather than a rail relaxation: the Linux absolute p50 ceiling
is tightened from 15000 to 4000 ms, three times the largest observed p50.

Windows has a different failure mode. Across the same seventeen comparisons,
all 102 samples split into a fast 1410-4919 ms mode and a slow 6492-21997 ms
mode. The baseline runs first on the shared hosted runner and its first sample
was slow in all seventeen runs, while only two of fifty-one candidate samples
were slow. With three samples nearest-rank p95 is the maximum, so the relative
gate is comparing which side paid the one-time browser/fixture bootstrap rather
than product latency. A reviewed 4500 ms floor covers the observed same-code
mode mismatch; the 20000 ms absolute p95 rail is unchanged. This is an
explicitly bounded workaround, not evidence that the slow mode is acceptable:
the measurement should eventually prime that exact refresh path before
sampling, after which the Windows floor can be recalibrated downward.

`environmentSlowMs` shows a superficially similar relative false positive and
is deliberately _not_ given a `relativeStatistic`, because the data shows the
remedy does not work there. Its maximum falls on the **first** sample in
thirty of thirty-eight Linux sample sets, not on a later one, and the residual
failure in run `35715515352` fails on p50 as well as p95. That pattern is a
missing warm-up rather than a tail artifact, and Windows - whose first IPC
costs tens of milliseconds rather than hundreds - shows no such skew. The fix
is the discarded warm-up probe described above. Simulating it over nineteen
runs by removing the first sample takes the relative failures from two to one;
that simulation compares two retained samples where the shipped harness keeps
three, so it bounds the direction of the effect rather than its exact size.
The remaining failure in `35715515352` is retained and not chased.

A performance failure is not cleared by repeating until a favorable sample is
found. One rerun is permitted only for an identified infrastructure failure,
such as a runner or driver crash, and the original failure artifact must remain
available. A metric-budget violation remains the result that reviewers see.

## Baseline and budgets

Pull requests build the candidate and the pull request's current base commit on
the same hosted runner. Main and scheduled runs compare the checked-out commit
with its first parent. A comparison is rejected rather than silently downgraded
when the baseline commit or any tracked compatibility field differs:

For the one-time bootstrap against a base that predates the isolated benchmark,
the workflow backports only the required e2e Marketplace/environment hooks and
repeatable browser cleanup into that base checkout. It does not copy candidate
application features. Once the contract exists on main, the baseline builds its
own checked-in implementation without this compatibility path.

- suite, platform, release profile, and package kind;
- OS, architecture, runner image, CPU model/core count, memory class, and
  WebView version;
- complete fixture and sampling policy.

Each gated metric must pass both the absolute safety ceiling and the relative
limit in [`performance/budgets.json`](../performance/budgets.json). The initial
relative limit is the baseline plus the larger of 20 percent or the tracked
unit noise floor. Linux floors are 50 ms, 32 MiB, one percentage point, and one
process; Windows floors are 75 ms, 64 MiB, one percentage point, and two
processes. A reviewed metric may declare a scoped unit-specific override; the
Linux cached-catalog p95 uses 75 ms after three same-host comparisons observed a
3–60 ms range, and the Linux idle `idleCpuPercent` and
`backgroundPollingCpuPercent` gates use two percentage points after twelve
product-unchanged runs moved their p95 by up to 3.41 points. Unlike the latency
phase, the idle phase measures one variant per job, so those two reports come
from two runner VMs. This keeps tiny or zero measurements from turning harmless
scheduler noise into an infinite percentage while 20–30 percent regressions
above that floor still fail. Release-tag MSI and NSIS verification repeats the
absolute ceiling check; the same-host relative comparison has already run
before merge.

Budget values cannot be changed with environment variables. A budget change
must update its rationale and dated evidence, reference the motivating issue or
measurement, and receive the review required by `CODEOWNERS`. Loosening a limit
solely to make a failing run green is not valid evidence.

## Reports, summaries, and trends

Reports conform to
[`performance-report.schema.json`](../schemas/performance-report.schema.json)
and include the measured/base commits, build profile, timestamps, OS and WebView
versions, CPU/core and memory metadata, fixture sizes, sampling rules, raw
samples, statistics, resources, and every gate comparison. The budget file has
its own versioned
[`performance-budget.schema.json`](../schemas/performance-budget.schema.json).

The workflow uploads JSON, screenshots, failure records, and installer logs from
`artifacts/e2e/release-performance`. It also writes actual, baseline, relative
change, absolute limit, sample count, p50, and p95 to the pull-request job
summary. Pull-request artifacts are retained for 30 days; main and the weekly
Monday trend run are retained for 90 days.

Run the deterministic measurement-tool tests locally with:

```bash
npm run test:perf
```

On a Linux desktop with `tauri-driver`, `WebKitWebDriver`, Xvfb, and Chrome
installed, build and measure the release executable with:

```bash
npm run test:perf:release:build
xvfb-run --auto-servernum node scripts/perf/release-webview.mjs \
  --application src-tauri/target/release/mendimaru \
  --package-kind release-executable
```

Comparing two reports uses the checked-in policy and fails on a schema,
baseline, host, fixture, absolute-budget, or relative-budget mismatch:

```bash
node scripts/perf/performance-gate.mjs candidate.json baseline.json
```
