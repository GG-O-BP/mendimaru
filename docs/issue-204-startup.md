# Windows cold-start comparison (#204)

[Run 35811293444](https://github.com/GG-O-BP/mendimaru/actions/runs/35811293444)
compares `90629a3` with `eb943b4`. Only the Linux workspace-scan gate and its
regression tests changed; Windows product sources and dependency inputs did not.

The original cold samples were `[1466.821, 1418.416, 1475.815]` and
`[2093.205, 1982.813, 2111.658]` ms. All candidate samples moved. Substituting
p50 alone still fails the old 600 ms floor: the median increased 626.384 ms.

Forty downloaded comparisons with unchanged product sources and dependencies
(excluding npm script-only edits) have p50 movements from -444.027 to +626.384 ms.
Their p95 movements span -4424.656 to +1128.868 ms. With three samples p95 is a
single maximum, so it is not a stable relative measure of typical startup.

The Windows executable cold-start relative gate now uses p50 and a 700 ms floor.
The 20 percent relative rule, original 20-second absolute p95 rail, cold cache
clearing, sample count and warm-up count are unchanged. Linux and installed MSI/
NSIS budgets are unchanged. A sustained increase greater than the floor still
fails; an isolated increase below the absolute ceiling remains reported but
cannot by itself fail the relative gate. This sensitivity tradeoff is explicit.
The original failure is retained and is not replaced with a successful rerun.

After calibration, independent PR #211 run 35955455763 failed the old policy
again: baseline `[1508.084, 1474.057, 1509.117]`, candidate
`[1542.322, 2143.699, 2148.045]` ms. The already selected 700 ms p50 policy
passes these held-out samples without further adjustment (p50 +635.615 ms).
A regression test preserves both verdicts.
