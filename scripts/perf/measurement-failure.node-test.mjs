import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  collectFailureReports,
  isFailureReportName,
  renderAnnotations,
  renderFailureSummary,
  reportMeasurementFailure,
  summariseFailure,
} from "./measurement-failure.mjs";

async function fixtureDirectory(files) {
  const directory = await mkdtemp(
    path.join(os.tmpdir(), "measurement-failure-"),
  );
  for (const [name, content] of Object.entries(files)) {
    await writeFile(
      path.join(directory, name),
      typeof content === "string" ? content : JSON.stringify(content),
    );
  }
  return directory;
}

const harnessFailure = {
  status: "failed",
  classification: "harness",
  reason: "webdriver-script-timeout",
  stage: "environment-timeout-recovery-samples",
  stageSample: "2/3",
  webdriverCommand: {
    method: "POST",
    endpoint: "/session/abc/execute/async",
    elapsedMs: 30001,
    requestTimeoutMs: 35000,
    scriptTimeoutMs: 30000,
  },
};

test("only failure reports are collected", () => {
  assert.equal(isFailureReportName("linux-baseline-idle.failure.json"), true);
  assert.equal(isFailureReportName("linux-baseline-idle.json"), false);
  assert.equal(isFailureReportName("skip-reason.txt"), false);
  assert.equal(isFailureReportName(undefined), false);
});

test("a missing directory yields no entries instead of throwing", async () => {
  const entries = await collectFailureReports(
    path.join(os.tmpdir(), `absent-${Date.now()}`),
  );
  assert.deepEqual(entries, []);
});

test("a directory without failure reports yields no entries", async () => {
  const directory = await fixtureDirectory({
    "linux-candidate-latency.json": { ok: true },
    "skip-reason.txt": "skipped",
  });
  assert.deepEqual(await collectFailureReports(directory), []);
});

test("multiple failure reports are ordered and deduplicated by name", async () => {
  const directory = await fixtureDirectory({
    "linux-candidate-latency.failure.json": harnessFailure,
    "linux-baseline-latency.failure.json": harnessFailure,
  });
  const entries = await collectFailureReports(directory);
  assert.deepEqual(
    entries.map((entry) => entry.name),
    [
      "linux-baseline-latency.failure.json",
      "linux-candidate-latency.failure.json",
    ],
  );
});

test("a harness failure is separated from a measured-contract failure", () => {
  const harness = summariseFailure({
    name: "linux-baseline-idle.failure.json",
    report: harnessFailure,
  });
  assert.equal(harness.classification, "harness");
  assert.equal(harness.reason, "webdriver-script-timeout");
  assert.match(harness.headline, /성능 판정 아님/);
  assert.match(harness.headline, /environment-timeout-recovery-samples/);
  assert.match(harness.headline, /sample 2\/3/);

  const measurement = summariseFailure({
    name: "linux-candidate-latency.failure.json",
    report: {
      classification: "measurement",
      reason: "assertion-failed",
      stage: "large-workspace-scan-samples",
    },
  });
  assert.equal(measurement.classification, "measurement");
  assert.match(measurement.headline, /측정 계약 위반/);
});

test("an unreadable or shapeless report degrades to an explicit unknown", async () => {
  const directory = await fixtureDirectory({
    "linux-baseline-idle.failure.json": "{ not json",
  });
  const [entry] = await collectFailureReports(directory);
  const summary = summariseFailure(entry);
  assert.equal(summary.classification, "unknown");
  assert.equal(summary.reason, "unreadable-failure-report");

  const shapeless = summariseFailure({ name: "x.failure.json", report: {} });
  assert.equal(shapeless.classification, "unknown");
  assert.equal(shapeless.reason, "unknown");
  assert.match(shapeless.headline, /an unrecorded stage/);
});

test("the summary states that no gate verdict was produced", () => {
  const rendered = renderFailureSummary([
    summariseFailure({
      name: "linux-baseline-idle.failure.json",
      report: harnessFailure,
    }),
  ]);
  assert.match(rendered, /성능 게이트는 이 잡에서 실행되지 않았습니다/);
  assert.match(rendered, /linux-baseline-idle\.failure\.json/);
  assert.match(rendered, /POST \/session\/abc\/execute\/async/);
  assert.match(rendered, /script deadline 30000/);
});

test("an empty summary still explains itself", () => {
  assert.match(
    renderFailureSummary([]),
    /분류된 하네스 실패 리포트가 없습니다/,
  );
});

test("annotations escape the characters GitHub treats as control sequences", () => {
  const [annotation] = renderAnnotations([
    summariseFailure({
      name: "x.failure.json",
      report: {
        classification: "harness",
        reason: "a%b\nc::d",
        stage: "idle-sampling",
      },
    }),
  ]);
  assert.match(annotation, /^::error title=/);
  assert.ok(!annotation.includes("\n"), "annotations must stay on one line");
  assert.match(annotation, /a%25b%0Ac%3A%3Ad/);
});

test("reporting emits one annotation per report and one summary", async () => {
  const directory = await fixtureDirectory({
    "linux-baseline-latency.failure.json": harnessFailure,
    "linux-candidate-latency.failure.json": harnessFailure,
  });
  const written = [];
  const result = await reportMeasurementFailure(directory, {
    write: (text) => written.push(text),
  });
  assert.equal(result.annotations.length, 2);
  assert.equal(written.length, 3);
  assert.match(written.at(-1), /## 측정 실패 분류/);
});

test("reporting an absent directory is not an error", async () => {
  const written = [];
  const result = await reportMeasurementFailure(
    path.join(os.tmpdir(), `absent-${Date.now()}-b`),
    { write: (text) => written.push(text) },
  );
  assert.deepEqual(result.entries, []);
  assert.equal(result.annotations.length, 0);
});
