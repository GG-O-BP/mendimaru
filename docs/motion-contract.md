# Motion contract

Motion is a tested application contract, not decorative implementation detail.
Changes to animation selectors, duration, iteration, or lifecycle must update
this document, `scripts/verify-motion-contract.mjs`, and the real Tauri WebKit
E2E together.

## Inventory and expected behavior

| Motion                      | Trigger                                                             | Expected lifetime                                                                                                     |
| --------------------------- | ------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `route-travel`              | The Linux-to-Windows route is online and `.route-packet` is mounted | Repeats while the route remains online; removed immediately when offline. It follows inline direction in LTR and RTL. |
| `spin`                      | A component explicitly adds `.spin` during pending work             | Repeats only while that pending state is rendered; the class or element is removed at completion.                     |
| `progress-shimmer`          | The installation progress fill has `.active`                        | Repeats only for non-terminal download/install states.                                                                |
| `progress-stage-pulse`      | The installation bar is `aria-busy="true"` and a stage is current   | Repeats only for non-terminal work.                                                                                   |
| Progress width transition   | Reported installation percentage changes                            | Interpolates the new width for 450 ms; no continuous idle work.                                                       |
| Navigation-arrow transition | The active navigation item changes                                  | Interpolates opacity/position for 150 ms.                                                                             |
| Button transition           | Hover/focus color or border changes                                 | Interpolates for 120 ms.                                                                                              |

`prefers-reduced-motion: reduce` collapses every animation and transition to a
single near-instant iteration. No motion may bypass that global fallback.

## Regression policy

- Do not change the online route animation to a bounded iteration count. Online
  is a live state, so a packet that permanently disappears falsely looks
  disconnected.
- Do not restart animations from E2E code. Tests must observe production motion
  naturally; restarting it can hide a broken lifecycle.
- Do not replace `.route-packet` with a tag selector. The semantic class is the
  stable contract between React, CSS, and WebDriver.
- Do not add a new CSS `animation:` declaration without adding it to the
  inventory and deciding whether it is allowed to run while idle.
- The real WebKit E2E may allow continuous `route-travel` only. Busy spinners,
  shimmer, and stage pulses must disappear when their operation finishes.

Run both gates after any visual or state-management change:

```bash
npm run test:motion
WEBKIT_DISABLE_DMABUF_RENDERER=1 xvfb-run --auto-servernum npm run test:e2e
```
