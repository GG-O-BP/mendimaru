import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { evaluateHold, renderHold } from "./regression-hold.mjs";
import { regressionLabel, revertCandidateLabel } from "./regression-issue.mjs";

const repository = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

function issue(overrides = {}) {
  return {
    number: 181,
    title: "idle gate 오탐",
    url: "https://example.test/issues/181",
    state: "OPEN",
    labels: [{ name: regressionLabel }],
    ...overrides,
  };
}

test("no open regression issue means no hold", () => {
  const decision = evaluateHold([]);
  assert.equal(decision.held, false);
  assert.match(renderHold(decision), /clear/);
});

test("one open labelled issue holds merges", () => {
  const decision = evaluateHold([issue()]);
  assert.equal(decision.held, true);
  assert.equal(decision.issues[0].number, 181);
  const rendered = renderHold(decision);
  assert.match(rendered, /ACTIVE/);
  assert.ok(rendered.includes("#181"));
  assert.ok(rendered.includes("https://example.test/issues/181"));
});

test("a revert candidate is called out in the hold message", () => {
  const decision = evaluateHold([
    issue({
      labels: [{ name: regressionLabel }, { name: revertCandidateLabel }],
    }),
  ]);
  assert.ok(renderHold(decision).includes("[revert candidate]"));
});

test("a closed issue releases the hold", () => {
  // Closing the issue is the documented way a human clears a false positive.
  assert.equal(evaluateHold([issue({ state: "CLOSED" })]).held, false);
  assert.equal(evaluateHold([issue({ state: "closed" })]).held, false);
});

test("removing the label releases the hold", () => {
  assert.equal(evaluateHold([issue({ labels: [] })]).held, false);
  assert.equal(
    evaluateHold([issue({ labels: [{ name: "bug" }] })]).held,
    false,
  );
});

test("a pull request can never hold merges", () => {
  // A PR carrying the label would otherwise block the branch it is trying to fix.
  assert.equal(evaluateHold([issue({ pull_request: {} })]).held, false);
  assert.equal(evaluateHold([issue({ isPullRequest: true })]).held, false);
});

test("plain string labels are understood too", () => {
  assert.equal(evaluateHold([issue({ labels: [regressionLabel] })]).held, true);
});

test("a malformed query result fails closed", () => {
  // Reading a broken response as "main is healthy" is the one outcome that
  // would quietly reopen the gap this check exists to close.
  assert.throws(() => evaluateHold(null), /expected an array/);
  assert.throws(() => evaluateHold({}), /expected an array/);
  assert.throws(() => evaluateHold("[]"), /expected an array/);
});

test("junk entries are discarded without breaking the decision", () => {
  const decision = evaluateHold([null, undefined, 7, issue()]);
  assert.equal(decision.held, true);
  assert.equal(decision.issues.length, 1);
});

test("an entry without a usable number cannot hold merges", () => {
  assert.equal(evaluateHold([issue({ number: undefined })]).held, false);
  assert.equal(evaluateHold([issue({ number: "abc" })]).held, false);
});

test("multiple holds are listed in issue order", () => {
  const decision = evaluateHold([
    issue({ number: 205 }),
    issue({ number: 181 }),
    issue({ number: 190 }),
  ]);
  assert.deepEqual(
    decision.issues.map((entry) => entry.number),
    [181, 190, 205],
  );
});

test("ci.yml runs the hold as its own reported check", () => {
  const workflow = readFileSync(
    path.join(repository, ".github", "workflows", "ci.yml"),
    "utf8",
  );
  assert.match(workflow, /^ {2}perf-regression-hold:$/m);
  assert.ok(workflow.includes("Post-merge performance hold"));
  assert.ok(workflow.includes("scripts/ci/regression-hold.mjs"));
  assert.ok(workflow.includes(`--label "${regressionLabel}"`));
  // Must not depend on the relevance classifier: a hold that a path filter can
  // skip is not a hold.
  const job = workflow.slice(workflow.indexOf("  perf-regression-hold:"));
  assert.ok(!job.includes("needs: changes"));
});
