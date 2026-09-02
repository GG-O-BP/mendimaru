# RustSec warning policy

Mendimaru denies every RustSec vulnerability, unknown warning, yanked lockfile
entry, expired exception, and stale exception. `cargo audit --json` is parsed by
`scripts/verify-rustsec-policy.mjs`; warnings never pass merely because
cargo-audit exits zero.

The current exception baseline is `security/rustsec-exceptions.json`. It was
reviewed against cargo-audit 0.22.2 and RustSec advisory database commit
`4b27756fe154d080be5c91a7486e4fbd5cfc3b3a` (1,236 advisories, last updated
2026-09-01). Each exception records an owner, introducing dependency, reason,
upstream HTTPS link, remediation plan, exact package/version, and expiry date.
An exception whose advisory disappears or whose package version changes is
reported as stale and fails CI.

## Exception classes

- **Unmaintained:** allowed only with the complete metadata above. The current
  GTK3 bindings, `proc-macro-error`, and `unic-*` entries are transitive
  dependencies of Tauri/wry/urlpattern and expire on 2026-12-31.
- **Unsound:** denied by default. The only current exception is
  `RUSTSEC-2024-0429` in `glib 0.18.5`, and it uses the stricter
  `bounded-reachability` class. Mendimaru source has no reference to the
  affected `VariantStrIter` API; the checked commit and evidence are embedded in
  the manifest. Any new source reference or absence of that evidence fails the
  policy. This exception expires on 2026-10-01.
- **Yanked:** no exception class. A yanked entry must be updated or removed.

The stale `chacha20 0.10.1` lockfile entry was updated to `0.10.2`, eliminating
the current yanked warning without changing the dependency graph.

## CI and review

CI and release verification run `actions/dependency-audit/action.yml`. The
action stores raw npm audit and cargo-audit JSON plus a machine-readable policy
result under `artifacts/dependency-audit/`, and writes a human-readable GitHub
step summary.

When changing `src-tauri/Cargo.lock`, run:

```sh
cargo audit --file src-tauri/Cargo.lock --json \
  | node scripts/verify-rustsec-policy.mjs
```

Update exception metadata and expiry during dependency review. Do not add an
ID without an owner, upstream link, introducing dependency, remediation plan,
and explicit expiry. The weekly scheduled workflow also creates a compatible
Cargo update PR after running the portable check suite.
