# Issue 219: slow probe and build-environment confounding

The original [run 35964497507](https://github.com/GG-O-BP/mendimaru/actions/runs/35964497507)
is preserved as failed. Linux `environmentSlowMs` has baseline samples
821.619/777.302/803.068 ms and candidate samples 829.382/996.823/846.997 ms.
Candidate p95 exceeds the 985.943 ms relative limit by 10.880 ms. No sample is
removed, and neither p95 nor its budget is changed.

The complete `1d3ccd4..0508e93` diff contains the functional CI workflow's
Windows Marketplace browser setup, AUR compiler-cache size, a cache-planner
test/fixture and documentation. None changes the standalone release build,
product, measurement harness or performance policy. The reporter now recognizes
these audited automation paths for both measured suites; an unknown path,
product/resource change, measurement workflow or budget change still retains
the revert signal. Failure issues and raw gate verdicts remain visible.

## The binaries were not identical

Artifact inspection on 2026-09-24 found a build-environment confound that the
earlier issue comment did not establish. Both sides were cache hits, but their
fingerprint salts differ in host glibc: baseline `2.39-0ubuntu8.9`, candidate
`2.39-0ubuntu8.8`. The keys are respectively `a80daf72c437a1dcd2f1079ad92e253a`
and `8ae513f7f98866728143f6d512eda5fa`.

| AppImage  | SHA-256                                                            |
| --------- | ------------------------------------------------------------------ |
| Baseline  | `901d965ec7f46b3293a9fd75d30cd5a0f770d50050e28603f58c1279771971cb` |
| Candidate | `c28cb115ab07f7067049a3bc7779752016668741712b423bce62b35b1eb299b6` |

Each AppImage was extracted at SquashFS offset 944632. Hashing the 466 regular
files found six differing payload files: `usr/bin/mendimaru` and five bundled
Kerberos/SQLite libraries. Their hashes are preserved with the complete original
candidate report, baseline slow samples and changed paths in
`scripts/ci/fixtures/issue-219.json`. This was not a byte-identical comparison;
unchanged repository sources alone do not imply identical build artifacts.

The source diff rules out attributing this result to the merged automation
change. The measurements do not distinguish scheduling variation from the
different build environments, and do not prove either explanation caused the
10.880 ms excess. Issue 219 is resolved as a wrongly attributed revert candidate,
not as evidence of improved latency or a passing original run. Build provenance
and steady-state measurement remain relevant when interpreting future results.

Validation replays the original report through the reporter and verifies that
actual WebView inputs still retain the revert label. Every original comparison,
sample, budget and gate result is unchanged; no favorable rerun is substituted.
