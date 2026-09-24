# Actions cache headroom (#209)

The 2026-09-24 Actions listing still contains the 9.06 GiB retained floor
reported in #209. It leaves less room than the 1.12 GiB largest single save.
Pruning obsolete generations alone cannot fix that layout.

## Shared Linux dependency cache

`Test (ubuntu-latest)` now restores the `rust-ubuntu` cache with `save-if: false`.
`Rust tests and clippy (ubuntu)` remains its only writer. This removes the
redundant `v0-rust-test-ubuntu-*` family without changing the existing writer key.

[CI run 35818611312](https://github.com/GG-O-BP/mendimaru/actions/runs/35818611312)
records identical toolchain/manifest suffixes (`6ff13d87-aea8b408`) and the same
absolute workspace target path in both jobs. The reader's filtered default-feature
Cargo tests are a subset of the writer's `cargo test --all-targets` and clippy.
The existing writer cache is 1,198,484,198 bytes; the redundant reader cache is
868,309,554 bytes. A single writer prevents a narrower cold build from winning
the immutable cache key before the complete build can save it.

The release-performance variants retain separate keys: their workspace paths or
feature sets differ. Registry pruning, offline Cargo builds, dropping a live
family and pruning during a measurement are not part of this change.

## Planner and retirement

The effective target is `min(9 GiB, cap - reserve)`. By default the reserve is the
largest entry in the complete listing, including entries proposed for deletion.
This only tightens the standing target. The plan reports the policy target,
reservation, remaining bytes, headroom and whether a single save fits. A reserve
at or above the cap is an error; insufficient headroom remains a warning.

Explicit retired-family prefixes reclaim caches that no current job writes.
They retain the fifteen-minute recent-use guard, ref-scoped retention, protection
for live main dependency caches and fail-closed metadata validation. The
workflow performs deletions only after a plan is produced; unknown open PRs do
not mean all PRs are closed. Tests couple the retirement list to the CI writers.

Read-only replay of the live 2026-09-24 listing gives:

| Measure                                   |  GiB |
| ----------------------------------------- | ---: |
| Before                                    | 9.06 |
| Redundant reader family                   | 0.81 |
| After planned retirement                  | 8.25 |
| Required maximum (cap minus largest save) | 8.88 |
| Remaining headroom                        | 1.75 |
| Largest observed save                     | 1.12 |

The plan has no headroom warning. This is a dry-run result: actual deletion and
post-merge convergence must be verified in `Actions cache budget`. PR and
post-merge run links and cache-hit/envelope observations are recorded on #209.

## Limits

One largest observed save fits; this is not a reservation system for simultaneous
saves, and newly enlarged caches can exceed the historical maximum. A concurrent
1.12 GiB save plus a 0.89 GiB AUR save would exceed 1.75 GiB of headroom.

On a cold generation only the writer can refill the shared family. Its failure
leaves the reader cold until a successful writer finishes. Changes to either
job's features, target, environment or workspace require rechecking the subset
assumption. Existing warm hits and the unchanged installer rebuild cache must
be checked in CI, not inferred solely from the layout.

## Bound the AUR archive after convergence measurement

The first post-merge run, [35955439579](https://github.com/GG-O-BP/mendimaru/actions/runs/35955439579),
restored the shared Rust dependency key in both jobs and completed the full CI/
performance envelope in about 14.25 minutes. Its AUR builder, however, wrote a
1.327 GiB sccache generation, up from 0.885 GiB. The retained floor became
8.69 GiB and the larger single-save reserve lowered the target to 8.67 GiB.
The new warning correctly exposed another capacity problem.

The disposable AUR builder now sets `SCCACHE_CACHE_SIZE=1G` instead of `3G`.
[sccache's local storage](https://github.com/mozilla/sccache/blob/v0.17.0/docs/Local.md)
supports this bound; its [LRU initialization](https://github.com/mozilla/sccache/blob/v0.17.0/src/lru_disk_cache/mod.rs)
applies capacity while loading restored entries in modification-time order.
This bounds what is archived, including previous generations inside that
archive. It does not change compiler flags, package contents or cache keys.

This can evict a useful entry if the active working set exceeds 1 GiB. The
actual PR AUR hit rate, package verification and post-merge cache size must be
recorded on #209 before treating it as accepted. The previous 3 GiB run had
579 Rust misses, so a successful outer Actions restore alone is insufficient
evidence of compiler reuse.
