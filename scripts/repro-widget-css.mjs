// Manual upstream reproducer. Run with Studio Pro's Windows Node executable.
// Creates disposable directories only; never edits an app or an installation.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const [toolchain, localParent, uncParent] = process.argv.slice(2);
assert.equal(process.platform, "win32", "run on Windows with Studio Pro Node");
assert(
  toolchain && localParent && uncParent,
  "supply Node tools, local and UNC parent directories",
);
assert(/^[a-z]:[\\/]/i.test(localParent), "local parent must be a drive path");
assert(
  /^\\\\[^\\]+\\[^\\]+/.test(uncParent),
  "UNC parent must be a share path",
);
const { rspack, CssExtractRspackPlugin } = await import(
  pathToFileURL(path.join(toolchain, "node_modules/@rspack/core/dist/index.js"))
);
const corePackage = JSON.parse(
  await fs.readFile(
    path.join(toolchain, "node_modules/@rspack/core/package.json"),
    "utf8",
  ),
);
const local = await fs.mkdtemp(path.join(localParent, "mendimaru-css-145-"));
const unc = await fs.mkdtemp(path.join(uncParent, "mendimaru-css-145-"));
const results = [];

for (const [name, parent, reference] of [
  ["local-absolute", local, "forward"],
  ["unc-forward", unc, "forward"],
  ["unc-native", unc, "native"],
  ["unc-relative", unc, "relative"],
  ["no-css", local, "none"],
]) {
  const directory = path.join(parent, name);
  await fs.mkdir(directory);
  const cssPath = path.join(directory, "widget.css");
  const importPath =
    reference === "relative"
      ? "./widget.css"
      : reference === "native"
        ? cssPath
        : cssPath.replace(/\\/g, "/");
  await fs.writeFile(
    cssPath,
    ".widget-css-probe { color: rgb(17, 34, 51); }\n",
  );
  await fs.writeFile(
    path.join(directory, "index.js"),
    `${reference === "none" ? "" : `import ${JSON.stringify(importPath)};`}\ndocument.querySelector('h1').textContent = 'Loaded';\n`,
  );
  await fs.writeFile(
    path.join(directory, "index.html"),
    '<!doctype html><link rel="icon" href="data:,"><link rel="stylesheet" href="dist/widgets.css"><h1 class="widget-css-probe">Loading</h1><script type="module" src="dist/index.js"></script>',
  );
  const compiler = rspack({
    context: directory,
    entry: { index: "./index.js" },
    mode: "development",
    devtool: false,
    experiments: { outputModule: true },
    output: {
      path: path.join(directory, "dist"),
      module: true,
      library: { type: "module" },
      filename: "[name].js",
      clean: true,
    },
    optimization: {
      runtimeChunk: "single",
      splitChunks: {
        chunks: "all",
        minSize: 1,
        cacheGroups: {
          styles: {
            test: /\.css$/,
            name: "widgets",
            type: "css/mini-extract",
            chunks: "all",
            enforce: true,
          },
        },
      },
    },
    module: {
      rules: [
        {
          test: /\.css$/i,
          use: [
            CssExtractRspackPlugin.loader,
            {
              loader: path.join(
                toolchain,
                "node_modules/css-loader/dist/index.js",
              ),
              options: { sourceMap: false, url: false },
            },
          ],
        },
      ],
    },
    plugins: [new CssExtractRspackPlugin({ filename: "widgets.css" })],
  });
  // Match F5's development watch compilation and a subsequent invalidation.
  const builds = [];
  try {
    await new Promise((resolve, reject) => {
      const timer = globalThis.setTimeout(
        () => watching.close(() => reject(new Error("watch build timed out"))),
        60000,
      );
      const watching = compiler.watch({}, async (error, stats) => {
        try {
          if (error) throw error;
          const info = stats.toJson({
            all: false,
            errors: true,
            warnings: true,
            assets: true,
            modules: true,
            cachedModules: true,
          });
          assert.equal(stats.hasErrors(), false, JSON.stringify(info.errors));
          const css = await fs
            .readFile(path.join(directory, "dist/widgets.css"))
            .catch((error) => {
              if (error.code === "ENOENT") return null;
              throw error;
            });
          builds.push({
            cssBytes: css?.length ?? 0,
            cssSha256: css
              ? createHash("sha256").update(css).digest("hex")
              : null,
            externalCss: info.modules.filter(
              (m) =>
                m.name?.startsWith("external ") &&
                m.name.includes("widget.css"),
            ).length,
            warnings: info.warnings.length,
          });
          if (builds.length === 1) {
            await fs.appendFile(
              cssPath,
              ".widget-css-probe { background-color: rgb(51, 34, 17); }\n",
            );
            watching.invalidateWithChangesAndRemovals(
              new Set([cssPath]),
              new Set(),
            );
          } else {
            watching.close((error) => {
              globalThis.clearTimeout(timer);
              if (error) reject(error);
              else resolve();
            });
          }
        } catch (error) {
          globalThis.clearTimeout(timer);
          watching.close(() => reject(error));
        }
      });
    });
  } finally {
    await new Promise((resolve, reject) =>
      compiler.close((error) => (error ? reject(error) : resolve())),
    );
  }
  results.push({ name, builds });
}
// Paths stay in a private sidecar for local browser verification.
await fs.writeFile(
  path.join(unc, "locations.json"),
  JSON.stringify({ local, unc }),
);
const report = {
  platform: process.platform,
  nodeVersion: process.version,
  rspackVersion: corePackage.version,
  results,
};
await fs.writeFile(
  path.join(unc, "report.json"),
  JSON.stringify(report, null, 2),
);
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
