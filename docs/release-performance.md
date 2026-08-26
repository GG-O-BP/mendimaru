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
  backend delay.
- `environmentTimeoutRecoveryMs`: time from a tracked 750 ms client deadline
  against a two-second delayed probe until the next normal environment IPC
  succeeds. The application is not restarted.
- `catalogCachedMs`: a disk-backed catalog read without starting a browser.
- `catalogRefreshMs`: a refresh from the isolated loopback Marketplace through
  a real sandboxed Chrome or Edge browser.
- `smallWorkspaceScanMs` and `largeWorkspaceScanMs`: repeated project discovery
  for the declared project counts and total byte sizes.
- `navigationMs`: WebDriver interaction through the Projects, Settings, and
  Studio Pro routes, ending when the target heading is visible.
- `backgroundPollingCpuPercent`: the first twelve five-second CPU samples from
  the long-idle window, kept separate to expose periodic polling.
- `idleCpuPercent`, `privateMemoryBytes`, `workingSetBytes`, and `processCount`:
  sixty five-second samples covering at least 300 seconds.
- `privateMemoryGrowthBytes`, `workingSetGrowthBytes`, and
  `processCountGrowth`: positive end-minus-start leak signals from that same
  window. Negative deltas are retained in `resources.delta` but become zero for
  the upper-bound leak metric.

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
3–60 ms range. This keeps tiny or zero measurements from turning harmless
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
