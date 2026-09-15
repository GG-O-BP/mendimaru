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

Until an upstream fix or a verified integration is available, use a clean local
Windows copy for this affected configuration and validate the actual widget
pages. General Linux browser support for UNC widget assets remains tracked by
[#63](https://github.com/GG-O-BP/mendimaru/issues/63). A Windows-local copy is an
explicit workflow workaround, not automatic project relocation by Mendimaru.
