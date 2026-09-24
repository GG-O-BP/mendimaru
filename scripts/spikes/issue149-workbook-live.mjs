// Stateful IronCalc operator fixture for #149. Run through the lease-owning
// Python driver beside this file; production browser suites do not add a
// file-upload API or an internal-model mutation API. Only getters inspect
// the real canvas calculation after native browser input.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";
import { promises as fs, fstatSync } from "node:fs";
import { createHash } from "node:crypto";
assert.equal(process.env.MENDIMARU_WORKBOOK_LEASED, "1");
const locks = JSON.parse(process.env.MENDIMARU_WORKBOOK_LOCK_FDS);
assert.equal(locks.length, 2);
locks.forEach((fd) => assert(fstatSync(fd).isFile()));
const binary = process.env.MENDIMARU_E2E_BINARY;
assert(binary && path.isAbsolute(binary));
const require = createRequire(
  path.resolve(
    path.dirname(binary),
    "../lib/mendimaru/browser/browser-runner.mjs",
  ),
);
const baseUrl = process.env.MENDIMARU_WORKBOOK_BASE_URL;
assert(
  baseUrl && ["localhost", "127.0.0.1"].includes(new URL(baseUrl).hostname),
);
const { chromium, expect: baseExpect } = require("@playwright/test");
const expect = baseExpect.configure({ timeout: 30000 });
const hash = (b) => createHash("sha256").update(b).digest("hex");
const report = {
  outcome: "failed",
  startedAt: new Date().toISOString(),
  interception: false,
  coordination:
    "shared VM lease plus exclusive app-data lease; no other participants",
  steps: [],
  errors: [],
};
const marker = await fs.readFile(process.env.MENDIMARU_E2E_BUILD_MARKER);
report.buildMarkerSha256 = hash(marker);
const browser = await chromium.launch({
  headless: true,
  chromiumSandbox: true,
});
const deadline = setTimeout(() => {
  browser.close().catch(() => {});
}, 285_000);
deadline.unref();
report.browserVersion = browser.version();
const context = await browser.newContext({
  viewport: { width: 1600, height: 1200 },
  acceptDownloads: true,
});
const page = await context.newPage();
page.setDefaultTimeout(30000);
page.on("pageerror", (e) =>
  report.errors.push({ type: "page", message: e.message }),
);
page.on("console", (m) => {
  if (m.type() === "error")
    report.errors.push({ type: "console", message: m.text() });
});
page.on("requestfailed", (r) => {
  if (r.failure()?.errorText !== "net::ERR_ABORTED")
    report.errors.push({
      type: "network",
      url: r.url(),
      error: r.failure()?.errorText,
    });
});
page.on("response", (r) => {
  if (r.status() >= 400)
    report.errors.push({ type: "http", status: r.status(), url: r.url() });
});
const root = page.locator(".ic-workbook-container");
async function readCells() {
  return root.evaluate((el) => {
    let f = el[Object.keys(el).find((k) => k.startsWith("__reactFiber$"))];
    for (let i = 0; f && i < 35; i++, f = f.return) {
      const m = f.memoizedProps?.model;
      if (m && typeof m.getFormattedCellValue === "function")
        return {
          selected: Array.from(m.getSelectedCell()),
          quantity: m.getCellContent(0, 2, 4),
          price: m.getCellContent(0, 2, 5),
          formula: m.getCellContent(0, 2, 6),
          amount: m.getFormattedCellValue(0, 2, 6),
        };
    }
    throw new Error("No mounted workbook model");
  });
}
async function editCell(row, column, value) {
  const [sheet, y, x] = (await readCells()).selected;
  assert.equal(sheet, 0);
  assert(Math.abs(y - row) + Math.abs(x - column) < 20);
  for (let i = 0; i < Math.abs(y - row); i++)
    await root.press(y < row ? "ArrowDown" : "ArrowUp");
  for (let i = 0; i < Math.abs(x - column); i++)
    await root.press(x < column ? "ArrowRight" : "ArrowLeft");
  assert.deepEqual((await readCells()).selected, [0, row, column]);
  const input = page.locator(".ic-formula-bar-editor-wrapper textarea");
  await input.click();
  await input.fill(value);
  await input.press("Enter");
}
try {
  await page.goto(baseUrl);
  await page.locator(".mx-name-openDataDemo").click();
  await expect(root).toBeVisible();
  await page.locator(".mx-name-resetRows").click();
  await expect(page.locator(".mx-name-saveCount")).toHaveText(
    "Explicit saves: 0",
  );
  await page.goto(baseUrl);
  await page.locator(".mx-name-openDataDemo").click();
  await expect(root).toBeVisible();
  await expect(
    page.getByText("Loaded Mendix rows in one evaluation batch.", {
      exact: true,
    }),
  ).toBeVisible();
  await editCell(2, 4, "3");
  await editCell(2, 5, "7");
  await expect.poll(async () => (await readCells()).amount).toBe("21");
  const edited = await readCells();
  assert.equal(edited.quantity, "3");
  assert.equal(edited.price, "7");
  assert.equal(edited.formula, "=D2*E2");
  report.steps.push({
    name: "cell-input-and-real-engine-calculation",
    ...edited,
  });
  await page
    .getByRole("button", { name: "Save to Mendix", exact: true })
    .click();
  await expect(page.locator(".mx-name-saveCount")).toHaveText(
    "Explicit saves: 1",
  );
  await expect(
    page.locator('.mx-name-committedRows [data-position="3,0"]'),
  ).toHaveText("12");
  await expect(
    page.locator('.mx-name-committedRows [data-position="4,0"]'),
  ).toHaveText("84.5");
  await expect(page.locator(".spread-ui__notice")).toHaveText(
    "Saved 0 cells · skipped 36 · failed 0 · workbook state persisted",
  );
  report.steps.push({
    name: "workbook-state-writeback",
    explicitSaves: 1,
    readonlyListAttributesUnchanged: true,
    limitation:
      "Mendix ListAttributeValue is readonly; edited cells are saved in the bound WorkbookState attribute, not individual row attributes.",
  });
  await page.screenshot({
    path: "evidence/showcase-stateful-saved.png",
    fullPage: true,
  });
  await page.goto(baseUrl);
  await page.locator(".mx-name-openDataDemo").click();
  await expect(
    page.getByText("Restored the persisted workbook state.", { exact: true }),
  ).toBeVisible();
  await expect.poll(async () => (await readCells()).amount).toBe("21");
  await expect(
    page.locator('.mx-name-committedRows [data-position="3,0"]'),
  ).toHaveText("12");
  report.steps.push({
    name: "new-page-restores-committed-workbook-state",
    ...(await readCells()),
  });
  const downloadEvent = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download", exact: true }).click();
  const download = await downloadEvent;
  await download.saveAs("evidence/exported-workbook.ic");
  assert.equal(await download.failure(), null);
  const bytes = await fs.readFile("evidence/exported-workbook.ic");
  assert(bytes.length > 100 && bytes.length < 10 * 1024 * 1024);
  report.steps.push({
    name: "download-workbook",
    bytes: bytes.length,
    sha256: hash(bytes),
  });
  await page.goto(baseUrl);
  await page.getByRole("button", { name: "빈 워크북", exact: true }).click();
  await expect(
    page.getByText("Started an empty workbook.", { exact: true }),
  ).toBeVisible();
  const chooserEvent = page.waitForEvent("filechooser");
  await page.getByRole("button", { name: "Import .ic", exact: true }).click();
  await (await chooserEvent).setFiles("evidence/exported-workbook.ic");
  await expect(
    page.getByText(
      "Imported workbook ready. Press Save to persist it to Mendix.",
      { exact: true },
    ),
  ).toBeVisible();
  await expect.poll(async () => (await readCells()).amount).toBe("21");
  const imported = await readCells();
  assert.equal(imported.quantity, "3");
  assert.equal(imported.price, "7");
  assert.equal(imported.formula, "=D2*E2");
  report.steps.push({ name: "file-chooser-import-roundtrip", ...imported });
  const invalidChooser = page.waitForEvent("filechooser");
  await page.getByRole("button", { name: "Import .ic", exact: true }).click();
  await (
    await invalidChooser
  ).setFiles({
    name: "invalid.ic",
    mimeType: "application/vnd.ironcalc.workbook",
    buffer: Buffer.from("invalid fixture"),
  });
  await expect(page.locator(".spread-ui__notice")).toContainText(
    "Import not applied:",
  );
  assert.equal((await readCells()).amount, "21");
  report.steps.push({
    name: "invalid-import-preserves-workbook",
    amount: "21",
  });
  await page.screenshot({
    path: "evidence/showcase-stateful-imported.png",
    fullPage: true,
  });
  assert.equal(
    hash(await fs.readFile(process.env.MENDIMARU_E2E_BUILD_MARKER)),
    report.buildMarkerSha256,
  );
  assert.deepEqual(report.errors, []);
  report.outcome = "passed";
} catch (error) {
  report.failure = error.message;
  await page
    .screenshot({
      path: "evidence/showcase-stateful-failure.png",
      fullPage: true,
    })
    .catch(() => {});
  await fs
    .writeFile("evidence/showcase-stateful-failure.html", await page.content())
    .catch(() => {});
  throw error;
} finally {
  clearTimeout(deadline);
  report.finishedAt = new Date().toISOString();
  await fs.writeFile(
    "evidence/showcase-stateful.json",
    JSON.stringify(report, null, 2) + "\n",
  );
  await context.close();
  await browser.close();
}
