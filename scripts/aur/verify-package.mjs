import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { lstat, readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

// Compare the extracted .pkg.tar.zst, not makepkg's intermediate staging tree.
const [source, installed] = process.argv
  .slice(2)
  .map((value) => path.resolve(value));
assert(
  source && installed,
  "usage: verify-package.mjs SOURCE EXTRACTED_PACKAGE",
);
const json = async (filename) => JSON.parse(await readFile(filename, "utf8"));
const config = await json(path.join(source, "src-tauri/tauri.conf.json"));
const lock = await json(path.join(source, "package-lock.json"));
const manifest = await json(path.join(source, "package.json"));
const metadata = await readFile(path.join(installed, ".PKGINFO"), "utf8");
assert.match(metadata, /^depend = nodejs>=22\.22\.2$/m);
assert.match(metadata, /^depend = nss$/m);
assert.equal(manifest.engines.node, ">=22.22.2");
assert((await lstat(path.join(installed, "usr/bin/mendimaru"))).mode & 0o111);

const expected = {};
const modules = {};
for (const [input, output] of Object.entries(config.bundle.resources)) {
  assert(output.startsWith("browser/"), `unexpected resource: ${output}`);
  const original = path.resolve(source, "src-tauri", input);
  Object.assign(expected, await inventory(original, output.replace(/\/$/, "")));
  if (output.startsWith("browser/node_modules/")) {
    const name = output.replace("browser/node_modules/", "").replace(/\/$/, "");
    const packaged = await json(path.join(original, "package.json"));
    assert.equal(
      packaged.version,
      lock.packages[`node_modules/${name}`].version,
    );
    modules[name] = packaged.version;
    const direct =
      manifest.devDependencies[name] ?? manifest.dependencies[name];
    if (direct)
      assert.equal(direct, packaged.version, `${name} must be pinned`);
    for (const [dependency, version] of Object.entries(
      packaged.dependencies ?? {},
    )) {
      assert.equal(
        lock.packages[`node_modules/${dependency}`].version,
        version,
      );
      assert(
        Object.values(config.bundle.resources).includes(
          `browser/node_modules/${dependency}/`,
        ),
        `missing transitive runtime dependency: ${dependency}`,
      );
    }
  }
}
const actual = await inventory(
  path.join(installed, "usr/lib/mendimaru/browser"),
  "browser",
);
assert.deepEqual(
  actual,
  expected,
  "packaged resources must match the locked Tauri resources byte for byte",
);
assert.equal(
  Object.keys(actual).some((name) => /(?:^|\/)\.local-browsers\//.test(name)),
  false,
);
console.log(
  JSON.stringify(
    {
      passed: true,
      minimumNodeVersion: "22.22.2",
      modules,
      files: actual,
    },
    null,
    2,
  ),
);

async function inventory(filename, relative) {
  const stat = await lstat(filename);
  assert(!stat.isSymbolicLink(), `resource must not be a symlink: ${relative}`);
  if (stat.isDirectory()) {
    const files = {};
    for (const name of (await readdir(filename)).sort()) {
      Object.assign(
        files,
        await inventory(path.join(filename, name), `${relative}/${name}`),
      );
    }
    return files;
  }
  assert(stat.isFile(), `resource must be a regular file: ${relative}`);
  return {
    [relative]: createHash("sha256")
      .update(await readFile(filename))
      .digest("hex"),
  };
}
