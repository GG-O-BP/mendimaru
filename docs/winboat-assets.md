# Ordinary Linux browsers and UNC widget imports (#63)

Studio Pro 11.12.3 can generate imports pointing at
`//host.lan/Data/<project>/deployment/web/widgets/...` for shared-workspace
projects. Rspack treats these as external URLs. An ordinary Linux browser then
requests `host.lan:80`, where neither DNS nor an HTTP asset server is configured.
A CSS import can also remain an external JavaScript import instead of entering
Rspack's CSS extraction pipeline. An automation-only asset mirror does not fix
this ordinary-browser path.

## Opt-in repair

Keep this foreground command running while working in Studio:

```bash
mendimaru project list --json
mendimaru assets watch --project-id project_REPLACE_WITH_ID --rewrite-generated-assets
```

In another terminal, open the **same selected project** normally with
`studio start --version 11.12.3 --project-id ...` (or the desktop application),
prepare Runtime forwarding as usual, and use Studio's Run Locally/F5 action.
Wait for Rspack's build to complete, then open the ordinary Runtime URL in Chrome.
If the watcher reports `normalized`, wait for Rspack's subsequent successful
build before reloading a page. If the inputs are already relative, no repair
event is needed; wait for the normal Studio build completion. A first load during
that interval may still observe the previous failed bundle and need a reload.

The command requires the explicit `--rewrite-generated-assets` opt-in. It changes
**only generated `.js` files under `deployment/web/layouts` and
`deployment/web/pages`**, replacing this selected project's static widget imports
with file-relative imports. For example:

```js
import "../widgets/com/mendix/widget/web/languageselector/LanguageSelector.css";
```

Rspack then bundles the real widget modules and extracts the real CSS. Mendimaru
does not patch `dist`, fabricate empty CSS, or disable browser errors. It does
not edit the model, widget packages/sources, JavaScript actions, project settings,
or `rspack.config.mjs`. No privileged port, hosts entry, proxy, or browser request
interception is needed. The existing fixed-port Runtime policy is unchanged.
For this issue's normalization alternative, validate the resulting **Runtime
URL and its bundled assets**, not the obsolete `http://host.lan/Data/...` URL.

## Lifetime and diagnostics

This is a Linux-only foreground command, independent of the Studio keeper. It
uses the configured shared workspace and an ID from `project list`; explicit
external projects and shares other than `\\host.lan\Data` are unsupported.
`MENDIMARU_CONFIG_DIR` and `MENDIMARU_CACHE_DIR` select the same configuration as
the other CLI commands. It does not launch/stop Studio, issue guest diagnostics,
change Compose, or recreate the VM.

The command emits NDJSON immediately and continues until Ctrl+C or SIGTERM. This
long-running stream has its own `assets.watch` status shape (schema version,
`ok`, `state`, `generatedAssetsRewriteEnabled`, safe `message`, and `counts`),
not the short-lived CLI command envelope. `watching` confirms the opt-in;
`normalized` reports rewritten file/import counts. `failed` exits 1 with an action;
invalid/missing options exit 2 and unsupported platforms exit 3. Normal shutdown
emits `stopped` and exits 0. Generic `--json`, `--timeout-seconds`, and backend
switches are not accepted; use `assets --help` for its complete options.

The watcher checks every 250 ms, requires unchanged metadata across two scans,
compares content/identity again before atomic replacement, and rediscovers
removed/regenerated deployment directories. Only changed files are read.
Directory descriptors anchor traversal; symlinks and hardlinked/nonregular
JavaScript files are rejected. Limits are eight nested directory levels, 10,000
entries, 8 MiB per JavaScript file, and 64 MiB of candidate source bytes per scan.
Unsafe paths in matched widget imports, invalid UTF-8, permission errors, and
exceeded limits stop the command visibly. Only single-line static imports in the
generated form (with a semicolon and `.js`, `.mjs`, or `.css` target) are supported;
other forms and quoted/commented lookalikes are left intact. Correct the reported condition
and restart it before F5. File-system regeneration can briefly race a browser
load; the command does not claim that its `normalized` event means Rspack has
already finished rebuilding.

Keep the command alive through rebuilds, including Clean Deployment. Stopping it
leaves existing generated repairs intact; a later Studio regeneration can restore
the defect until the command runs again. Stop the command and rebuild from Studio
to return entirely to Studio-generated output. No restoration of original model
or widget files is necessary.

## Verification

The ordinary Rust suite checks opt-in parsing, correct nested relative imports,
source/dist preservation, unchanged-file scans, repeated generation, full
replacement of deployment, and traversal/symlink/hardlink/size rejection.
Actual Studio F5 and ordinary-browser evidence is recorded in
[PR #162](https://github.com/GG-O-BP/mendimaru/pull/162). A fixture or intercepted
browser success must not be described as live application success.
