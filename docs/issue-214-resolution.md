# Issue 214: installed CPU failure attribution

The original [run 35958148210](https://github.com/GG-O-BP/mendimaru/actions/runs/35958148210)
remains failed. Its MSI idle CPU p95 is 2.069%, versus 0% baseline and a
1 percentage point relative limit. Fifty-five of sixty candidate samples are
zero; the five nonzero values occur at indices 0, 1, 5, 6 and 7. This is evidence
of early activity, not proof of which process caused it or of steady-state health.

On 2026-09-24 both original installer artifacts were downloaded and hashed.
Baseline and candidate are byte-identical:

| Package | SHA-256                                                            |
| ------- | ------------------------------------------------------------------ |
| MSI     | `ee12ceb3f6c9c2d815049c1e8384f1148cd955a6d25a8edf16be6a18c27cab6a` |
| NSIS    | `8b39e25ffbd565c66101798b0025f7c273627067771630b405047931e4b517c6` |

Both build logs restored `bundle-installers-d49cfbe60f16b36e551d084b0ba4dd4c`.
The complete diff changes a WebView latency budget, its test/fixture and its
evidence document. Installed measurement code and budgets are unchanged.
Reverting that latency policy cannot repair this installed CPU measurement.
Issue 214 is therefore resolved as incorrect attribution to this merge, using
the issue's documented false-positive resolution path. The measurement itself
is not reclassified as passing. The original NSIS gate was not evaluated after
the MSI failure and is not claimed to have passed.

The reporter now checks a conservative, explicit list of unrelated inputs before
adding `revert-candidate`. It still creates the failure issue and quotes every
violation. Unknown files/suites, missing verdicts, unreadable diffs and revision
mismatches keep the original attribution behavior. Existing labels are never
automatically removed. The same decision governs rerun comments.

The original candidate report, baseline CPU series, exact changed paths and
installer hashes are retained in `scripts/ci/fixtures/issue-214.json`. Tests
replay its attribution and reject unsafe exclusions. No budget, sample count,
measurement window, product code or performance result changed. Startup versus
idle resource measurement remains a separate investigation in #195.
