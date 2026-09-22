import assert from "node:assert/strict";
import { test } from "node:test";
import {
  correctionReport,
  validateSuiteShape,
} from "./studio-f5-parity-gate.mjs";

const suite = (overrides) => ({
  schemaVersion: "1.0.0",
  name: "parity",
  beforeEach: [{ action: "goto", path: "/" }],
  tests: [
    {
      name: "cell edit computes",
      steps: [
        {
          action: "fill",
          locator: { by: "label", value: "Quantity" },
          value: "3",
        },
        {
          action: "expectText",
          locator: { by: "label", value: "Total" },
          value: "9",
        },
      ],
    },
  ],
  ...overrides,
});

test("parity suites need a state change and a value assertion", () => {
  assert.doesNotThrow(() => validateSuiteShape(suite(), "suite.json"));
  assert.doesNotThrow(() =>
    validateSuiteShape(
      suite({
        beforeEach: [
          {
            action: "click",
            locator: { by: "role", role: "button", name: "Add" },
          },
        ],
        tests: [
          {
            name: "t",
            steps: [
              {
                action: "expectValue",
                locator: { by: "label", value: "Rows" },
                value: "1",
              },
            ],
          },
        ],
      }),
      "suite.json",
    ),
  );
});

test("pure navigation suites cannot support widget-usability claims", () => {
  assert.throws(
    () =>
      validateSuiteShape(
        suite({
          tests: [
            {
              name: "t",
              steps: [
                {
                  action: "expectVisible",
                  locator: { by: "role", role: "heading" },
                },
              ],
            },
          ],
        }),
        "suite.json",
      ),
    /state-changing action/,
  );
  assert.throws(
    () =>
      validateSuiteShape(
        suite({
          tests: [
            {
              name: "t",
              steps: [
                {
                  action: "click",
                  locator: { by: "role", role: "button", name: "Go" },
                },
              ],
            },
          ],
        }),
        "suite.json",
      ),
    /value assertion/,
  );
  assert.throws(
    () => validateSuiteShape({ schemaVersion: "2.0.0" }, "suite.json"),
    /schema/,
  );
});

test("the host-lan correction must be reported before it can be judged", () => {
  const correction = correctionReport({
    corrections: [
      { kind: "host-lan-asset-mirror", applied: false, interceptedRequests: 0 },
    ],
  });
  assert.equal(correction.applied, false);
  assert.throws(
    () => correctionReport({ corrections: [] }),
    /must report the host-lan-asset-mirror correction/,
  );
});
