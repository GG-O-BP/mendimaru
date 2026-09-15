# Missing Mendix widget CSS (#145)

`mendix_widget_css_missing` means a same-origin stylesheet request ending in
`/dist/widgets.css` returned **404**. The browser report keeps the HTTP status,
URL without query parameters, and `http-error-status` reason. It adds an
actionable hint to the network artifact and to a failed test's summary, including
when a console error or a missing page heading is the first failure.

This is an observation, not automatic attribution to WinBoat or a particular
Mendix version. A missing aggregate can have several causes. Strict console and
network policies still fail; the runner does not synthesize CSS, intercept this
request, modify the project, or change policy settings.

## Check the producer before changing the consumer

1. Retain the failing browser artifacts, exact Studio Pro version, clean-build
   log, generated `deployment/web/rspack.config.mjs`, and widget imports from
   generated pages/layouts. Keep project data and credentials private.
2. Inspect the relevant MPK as a ZIP archive. Compare its CSS with the extracted
   file under `deployment/web/widgets/`. Missing or mismatched package content
   belongs to the widget/package producer.
3. Inspect the generated imports and `deployment/web/dist/widgets.css`. An import
   beginning `//host.lan/Data/` is a protocol-relative web URL. Resolving its DNS
   does not turn a CSS **JavaScript import** into an extracted stylesheet.
4. With the same Studio Pro version and unchanged source/MPKs, clean-build a
   **copy** on a Windows local drive. Stop the first app before starting the
   second on the same Runtime port. Compare aggregate content, generated imports,
   widget appearance, and strict diagnostics after F5 and a rebuild.
5. If both builds omit CSS despite valid local CSS imports, retain that separate
   reproduction. Check whether the app imports any widget CSS at all: configuring
   an extraction filename does not itself create a file without CSS input.

Do not create an empty `widgets.css`, concatenate arbitrary package files, or
turn off error policies as a repair. Those approaches cannot prove correct
styles, asset URL resolution, or cascade order.

## Upstream report: Studio Pro 11.12.3 UNC imports

**Owner:** Mendix Studio Pro's generated web-client imports and bundler
integration. Mendimaru owns the diagnostic/support boundary. The observed MPKs
contain their CSS and the deployment extraction preserves its bytes.

In the retained IronCalcSpreadUIShowcase F5 deployment, a generated layout imports
LanguageSelector CSS using `//host.lan/Data/.../LanguageSelector.css`. The
generated configuration combines CSS extraction with a `widgets` split-chunk
group and `filename: "widgets.css"`. It also excludes CSS from the generic copy
plugin. The React client unconditionally adds a stylesheet link to
`dist/widgets.css`.

Rspack treats protocol-relative web imports as external modules, bypassing CSS
extraction. When all widget CSS imports take that path, no aggregate is emitted.
The browser then requests a missing aggregate and also attempts to import CSS as
JavaScript. These symptoms explain why an asset mirror alone cannot establish
correct widget styles. See Rspack's
[external presets](https://rspack.dev/config/externals#externalspresets) and
[CSS extraction documentation](https://rspack.dev/plugins/css-extract-rspack-plugin).

### Minimal reproduction without a Mendix model

Run [repro-widget-css.mjs](../scripts/repro-widget-css.mjs) on Windows using the
**Node executable shipped with the affected Studio Pro installation**. Supply
the installation's Node tools directory, an existing local parent directory,
and an existing writable UNC parent directory:

```powershell
& 'C:\Program Files\Mendix\11.12.3\modeler\tools\node\win-x64\node.exe' `
  .\scripts\repro-widget-css.mjs `
  'C:\Program Files\Mendix\11.12.3\modeler\tools\node' `
  $env:TEMP '\\host.lan\Data'
```

Use the actual `node.exe` location in that installation. The script creates
unique disposable directories and uses the installed Rspack and CSS loader with
the relevant generated F5 configuration. It compares local absolute, forward
slash UNC, native UNC, relative UNC, and no-CSS inputs in development watch mode
and after a CSS change. `report.json` contains versions, CSS sizes/hashes, external
CSS module counts, and warning counts. `locations.json` contains private local
paths for inspecting the generated files; do not publish that sidecar.

Expected upstream fix: emit filesystem-resolvable widget imports (for example,
relative imports) when the project is on UNC storage, preserving CSS extraction
and the same behavior after Studio regenerates output. Also handle the separate
no-CSS-input case consistently with the client's unconditional aggregate link.
A generated-file edit is a diagnostic experiment; Studio can overwrite it.

### Verified results (2026-09-15 UTC)

The [evidence summary](issue-145-evidence.json) records the versions, hashes,
CSS sizes, browser results, and private raw-report hashes. In an isolated Windows
VM, Studio Pro **11.12.3** F5 built UNC and Windows-local copies of
IronCalcSpreadUIShowcase. Their model-file hashes and all **34 MPK hashes** match
after both builds. No widget or model edits were needed.

| Actual F5 build     | Aggregate CSS             | Strict Linux Chromium result                                                                     |
| ------------------- | ------------------------- | ------------------------------------------------------------------------------------------------ |
| UNC share           | Absent; HTTP 404          | Home fails; the network artifact and failed-heading summary include `mendix_widget_css_missing`. |
| Windows local drive | 111,215 bytes; HTTP 200   | Home and practical widget sample pass (2/2), with zero console, network, or page errors.         |
| Local F5 rerun      | Same CSS size and SHA-256 | Studio's log confirms watch resume and rebuild; both browser tests pass again (2/2).             |

The local aggregate contains both LanguageSelector and IronCalc rules. Computed
styles confirm LanguageSelector's 14 px font and 6 px right margin, and an
IronCalc button's 6 px border radius and flex display. The widget renders its
sample and reloads it. These browser runs use the direct loopback URL without
asset interception or a mirror.

The separate minimized watch comparison used the installation's Node **24.10.0**
and Rspack **1.7.11**. Local absolute, native UNC and relative UNC imports produced
47-byte CSS, then 104-byte CSS after the test stylesheet changed. Forward-slash
UNC imports remained external and produced no CSS in either build. The no-CSS
control also produced no aggregate. Chromium confirmed the initial and added
styles for all three successful cases; negative controls retained their failures.

Validation covers the named pages and configuration. The local F5 build also
reports a `DatagridDateFilter` `findDOMNode` linking warning; that widget's page
was not exercised here. This report does not certify all widget compatibility or
general UNC browser support. The report and reproducer are ready for a Mendix
Support submission; no external support ticket has been filed by this change.

For this UNC-import configuration, [the opt-in generated-import watcher](winboat-assets.md)
from #63 keeps imports relative across Studio regeneration and lets Rspack extract
the real CSS. Its separate live validation covers ordinary Linux Chrome after F5
and Clean Deployment plus F5. Follow its supported-scope and build-completion
instructions; it does not repair other causes of a missing aggregate.
A clean Windows-local copy remains an explicit alternative workflow; Mendimaru
does not automatically relocate projects.
