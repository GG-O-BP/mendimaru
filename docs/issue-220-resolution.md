# Issue 220: distinguish package verdicts and merge attribution

The [original run 35965392155](https://github.com/GG-O-BP/mendimaru/actions/runs/35965392155)
remains failed. Its NSIS working-set p95 was 508,809,216 bytes, above 467,755,008,
and process-count p95 was 12, above 10. Both are relative violations. The only
changed file was `docs/actions-cache-budget.md`; the product, measurement harness
and performance budgets are unchanged.

The original baseline/candidate MSI and NSIS artifacts were downloaded and hashed
on 2026-09-24. Each pair is byte-identical, and also matches the installers from
#214: MSI `ee12ceb3f6c9c2d815049c1e8384f1148cd955a6d25a8edf16be6a18c27cab6a`,
NSIS `8b39e25ffbd565c66101798b0025f7c273627067771630b405047931e4b517c6`.

The candidate NSIS process samples begin 9/12/16/17/17, then fall to seven.
The last five are all seven. Working set starts around 400 MB, peaks above
1 GB and ends at 157–163 MB. The MSI p95 is 383,537,152 bytes with eight
processes. This is the same startup-transient interpretation problem tracked in
#195; the raw data does not establish exactly which descendant caused it.
Reverting the cache documentation cannot repair this measurement. Issue 220 is
resolved as incorrect merge attribution, not as a passing original gate or as
completion of #195's steady-state acceptance criteria.

The reporter now includes package kind on every violation row and rerun comment,
so NSIS failures are distinguishable from MSI failures. Both installer gates run
after successful input validation, even when the first package's budget fails.
Cancellation, invalid/missing inputs and deferred PR measurements still skip
gating, and any failed package keeps the job red. This also prevents #214's MSI
failure from hiding the later NSIS verdict on future runs.

The original NSIS report, baseline resource samples, complete changed-path list
and installer hashes are retained in `scripts/ci/fixtures/issue-220.json`.
Regression tests preserve both violations, package identity and conservative
attribution. No product code, budget, sample count or measurement window changes.
