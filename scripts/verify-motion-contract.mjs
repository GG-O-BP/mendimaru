import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const root = path.resolve(import.meta.dirname, "..");
const cssFiles = (await walk(path.join(root, "src")))
  .filter((file) => file.endsWith(".css"))
  .sort();
const sources = new Map(
  await Promise.all(
    cssFiles.map(async (file) => [
      relative(file),
      await readFile(file, "utf8"),
    ]),
  ),
);

const animationDeclarations = [];
const keyframes = [];
const animationLonghands = [];
for (const [file, source] of sources) {
  for (const match of source.matchAll(/\banimation\s*:\s*([^;]+);/g)) {
    animationDeclarations.push(
      `${file}: ${match[1].replace(/\s+/g, " ").trim()}`,
    );
  }
  for (const match of source.matchAll(/\banimation-[a-z-]+\s*:\s*([^;]+);/g)) {
    animationLonghands.push(`${file}: ${match[0].replace(/\s+/g, " ").trim()}`);
  }
  for (const match of source.matchAll(/@keyframes\s+([a-z0-9-]+)/gi)) {
    keyframes.push(match[1]);
  }
}

assert.deepEqual(animationDeclarations.sort(), [
  "src/features/studio/installation-progress.css: progress-shimmer 1.35s ease-in-out infinite",
  "src/features/studio/installation-progress.css: progress-stage-pulse 1.4s ease-in-out infinite",
  "src/styles/feedback.css: spin 850ms linear infinite",
  "src/styles/workspace.css: route-travel 2.6s ease-in-out infinite",
]);
// Longhand `animation-*` properties can silently override the shorthand's
// iteration count, duration, or play-state and re-break a motion (for example
// `animation-iteration-count: 2` next to an `infinite` shorthand). The only
// legitimate longhands are the reduced-motion fallback, so anything else is a
// regression that the shorthand allowlist above cannot see.
assert.deepEqual(animationLonghands.sort(), [
  "src/styles/responsive.css: animation-duration: 0.01ms !important;",
  "src/styles/responsive.css: animation-iteration-count: 1 !important;",
]);
assert.deepEqual(keyframes.sort(), [
  "progress-shimmer",
  "progress-stage-pulse",
  "route-travel",
  "spin",
]);

const workspace = source("src/styles/workspace.css");
assert.match(
  workspace,
  /\.route-status\.online \.route-packet\s*\{[^}]*animation:\s*route-travel 2\.6s ease-in-out infinite;/s,
);
assert.match(
  workspace,
  /:root\[dir="rtl"\] \.route-track\s*\{[^}]*--route-travel-distance:\s*clamp\(-58px, -4vw, -18px\);/s,
);
assert.match(
  workspace,
  /@keyframes route-travel\s*\{[\s\S]*transform:\s*translateX\(var\(--route-travel-distance\)\);[\s\S]*\}/,
);

const shell = await readFile(path.join(root, "src/app/AppShell.tsx"), "utf8");
assert.match(
  shell,
  /\{online && \([\s\S]*className="route-packet"[\s\S]*data-testid="route-packet"/,
);

const progress = source("src/features/studio/installation-progress.css");
assert.match(
  progress,
  /\.progress-track > span\s*\{[^}]*transition:\s*width 450ms cubic-bezier\(0\.22, 1, 0\.36, 1\);/s,
);
assert.match(
  progress,
  /\.progress-track > span\.active::after\s*\{[^}]*animation:\s*progress-shimmer 1\.35s ease-in-out infinite;/s,
);
assert.match(
  progress,
  /\.download-bar\[aria-busy="true"\] \.progress-stages span\.current i\s*\{[^}]*animation:\s*progress-stage-pulse 1\.4s ease-in-out infinite;/s,
);

const feedback = source("src/styles/feedback.css");
assert.match(
  feedback,
  /\.spin\s*\{[^}]*animation:\s*spin 850ms linear infinite;/s,
);

const shellCss = source("src/styles/shell.css");
assert.match(shellCss, /\.nav-arrow\s*\{[^}]*transition:\s*150ms ease;/s);
const components = source("src/styles/components.css");
assert.match(
  components,
  /\.button\s*\{[\s\S]*transition:\s*color 120ms ease,\s*background 120ms ease,\s*border-color 120ms ease;/,
);

const responsive = source("src/styles/responsive.css");
assert.match(
  responsive,
  /@media \(prefers-reduced-motion: reduce\)\s*\{[\s\S]*animation-duration:\s*0\.01ms !important;[\s\S]*animation-iteration-count:\s*1 !important;[\s\S]*transition-duration:\s*0\.01ms !important;/,
);

const nativeE2e = await readFile(
  path.join(root, "scripts/test-tauri-e2e.mjs"),
  "utf8",
);
assert.doesNotMatch(nativeE2e, /marker\.style\.animation/);
assert.match(
  nativeE2e,
  /assert\.equal\(routeMotion\?\.iterationCount, "infinite"\)/,
);
assert.match(
  nativeE2e,
  /infiniteAnimations:\s*\[\s*\{ animationName: "route-travel", isOnlineRoute: true \},\s*\]/,
);

process.stdout.write(
  `Motion contract verified: ${animationDeclarations.length} CSS animations, no stray animation longhands, scoped busy motion, persistent online route motion, transitions, RTL, and reduced-motion fallback.\n`,
);

function source(file) {
  const value = sources.get(file);
  assert.ok(value, `missing motion source: ${file}`);
  return value;
}

async function walk(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const resolved = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(resolved)));
    else files.push(resolved);
  }
  return files;
}

function relative(file) {
  return path.relative(root, file).split(path.sep).join("/");
}
