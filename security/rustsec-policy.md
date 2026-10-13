# RustSec warning policy

Mendimaru denies every RustSec vulnerability, unknown warning, yanked lockfile
entry, expired exception, and stale exception. `cargo audit --json` is parsed by
`scripts/verify-rustsec-policy.mjs`; warnings never pass merely because
cargo-audit exits zero.

The current exception baseline is `security/rustsec-exceptions.json`. It was
reviewed on 2026-10-13 against cargo-audit 0.22.2 and RustSec advisory database
commit `57dcc0f2192e025138907a495c1ac9a7e1e3e123` (1,307 advisories, last
updated 2026-10-12). Each exception records an owner, introducing dependency,
reason, upstream HTTPS link, remediation plan, exact package/version, and expiry
date. An exception whose advisory disappears or whose package version changes is
reported as stale and fails CI.

The 2026-10-13 review (#232) handled two advisories published after the #228
review without adding an exception. `RUSTSEC-2024-0449` (unsafe-libyaml
unmaintained) is removed by replacing the deprecated `serde_yaml 0.9` with its
API-compatible, maintained successor `yaml_serde` from the YAML Organization.
`RUSTSEC-2026-0338` through `RUSTSEC-2026-0342` in `quinn-proto 0.11.16` are
removed by the compatible lockfile update to `0.11.19`; `quinn` is only an
optional, uncompiled HTTP/3 dependency of `reqwest`, but cargo-audit scans every
lockfile entry.

The 2026-10-12 review (#228) found the unchanged lockfile still reports zero
vulnerabilities and the same seven warnings, all covered by the exceptions
below. The earlier 2026-09-11 review removed the ten stale GTK3 unmaintained
exceptions `RUSTSEC-2024-0411` through `RUSTSEC-2024-0420`.

## Exception classes

- **Unmaintained:** allowed only with the complete metadata above. The current
  `proc-macro-error` and `unic-*` entries are transitive
  dependencies of Tauri/wry/urlpattern and expire on 2026-12-31.
- **Unsound:** denied by default. The only current exception is
  `RUSTSEC-2024-0429` in `glib 0.18.5`, and it uses the stricter
  `bounded-reachability` class. The affected functions are
  `glib::VariantStrIter::{next, nth, last, next_back, nth_back}`; the iterator
  is only created by `glib::Variant::array_iter_str()`.
  - _Upgrade path (re-checked 2026-10-12):_ not yet available. The newest
    `tauri` (2.12.3), `wry` (0.57.0) and `webkit2gtk` (2.0.2) still require
    `gtk 0.18` / `glib 0.18` on Linux, and `glib 0.18` has no patched release.
  - _Reachability (re-checked 2026-10-12 at
    `cdd66b5c738eb86979f7f99631409f358367d061` (#231 follow-up review)):_
    Mendimaru source has no reference to `VariantStrIter`, `variant_str_iter`
    or `array_iter_str`. The review also covered the locked sources of every
    crate that depends on `glib 0.18.5` (the GTK/GDK/GIO/Pango/Cairo bindings,
    `webkit2gtk`, `javascriptcore-rs`, `soup3`, `wry`, `tao`, `muda`,
    `tray-icon`, `libappindicator`, `tauri`, its runtimes and the
    dialog/fs/opener plugins); the Cargo metadata inverse closure comprises 23 packages including the app; none calls the affected API, and inside `glib`
    itself `array_iter_str()` is only used by its own tests.
  - Any new source reference or absence of that evidence fails the policy. This
    exception now expires on 2026-12-31, together with the unmaintained
    exceptions, so the next review re-checks the Tauri/wry upgrade path.
- **Yanked:** no exception class. A yanked entry must be updated or removed.

The stale `chacha20 0.10.1` lockfile entry was updated to `0.10.2`, eliminating
the current yanked warning without changing the dependency graph.

## CI and review

CI and release verification run `actions/dependency-audit/action.yml`. The
action stores raw npm audit and cargo-audit JSON plus a machine-readable policy
result under `artifacts/dependency-audit/`, and writes a human-readable GitHub
step summary.

On pull requests the full audit is gated by path relevance: it is skipped when
no npm or Cargo manifest, RustSec policy, or audit action path changed. An
exception expiry, however, is caused by the calendar and not by a changed path
(#228). The skipped gate therefore still runs the date-only check

```sh
node scripts/verify-rustsec-policy.mjs --expiry-only
```

which reads only the policy manifest, fails on an expired or invalid
`expiresAt`, and needs neither npm nor cargo-audit. Both the full verdict and
the expiry-only check print a GitHub `::warning::` for every exception that
expires within 30 days (`--warning-days` overrides the window), so the owner
can re-review an exception before `main` turns red.

When changing `src-tauri/Cargo.lock`, run:

```sh
cargo audit --file src-tauri/Cargo.lock --json \
  | node scripts/verify-rustsec-policy.mjs
```

Update exception metadata and expiry during dependency review. Do not add an
ID without an owner, upstream link, introducing dependency, remediation plan,
and explicit expiry. Extending an expiry is an owner decision that requires a
fresh review: re-check the upgrade path and, for an unsound exception, re-run
the reachability review and record its commit in `checkedCommit`.

The weekly scheduled workflow also creates a compatible Cargo update PR after
running the portable check suite. A compatible update can resolve an advisory;
its exception is then stale. Before verification the workflow runs

```sh
node scripts/verify-rustsec-policy.mjs --prune-stale --report cargo-audit.json
```

which removes only exceptions the audit no longer reports (#237). It never adds,
extends or edits an exception, and the full policy still fails on
vulnerabilities, unknown warnings, expiry, package drift and yanked entries.
The pruned manifest is committed with the lockfile so the owner reviews both in
the same PR.
