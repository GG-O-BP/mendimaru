import assert from "node:assert/strict";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { test } from "node:test";
import {
  correctionReport,
  runParityGate,
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

test(
  "a borrowed preparation survives successful and failed release checks",
  {
    skip: process.platform !== "linux",
  },
  async () => {
    const temporary = await fs.mkdtemp(
      path.join(os.tmpdir(), "parity-shared-"),
    );
    const shared = `shared_${"1".repeat(32)}`;
    const runtime = `runtime_${"2".repeat(32)}`;
    const preparationId = `preparation_${"3".repeat(32)}`;
    const settings = {
      MENDIMARU_STUDIO_PARITY_BINARY: path.join(temporary, "unused-binary"),
      MENDIMARU_STUDIO_PARITY_SUITE: path.join(temporary, "suite.json"),
      MENDIMARU_STUDIO_PARITY_EVIDENCE: path.join(temporary, "evidence.json"),
      MENDIMARU_STUDIO_PARITY_SHARED_SESSION_ID: shared,
    };
    const previous = Object.fromEntries(
      Object.keys(settings).map((key) => [key, process.env[key]]),
    );
    Object.assign(process.env, settings);
    await fs.writeFile(
      settings.MENDIMARU_STUDIO_PARITY_SUITE,
      JSON.stringify(suite()),
    );
    try {
      for (const passes of [true, false]) {
        const calls = [];
        const run = async (_binary, args) => {
          calls.push(args);
          if (args[0] === "browser" && args[1] === "session") {
            return {
              exitCode: 0,
              envelope: {
                data: {
                  state: "ready",
                  runtimeSessionId: runtime,
                  identity: {
                    runtimeMode: "studio-run-locally",
                    studioSessionId: "studio-10-20",
                  },
                  preparation: { comparable: true, preparationId },
                },
              },
            };
          }
          if (args[0] === "runtime" && args[1] === "wait")
            return { exitCode: 0, envelope: {} };
          assert.deepEqual(args.slice(0, 4), [
            "browser",
            "test",
            "--shared-session-id",
            shared,
          ]);
          const assisted = args[args.indexOf("--asset-mirror") + 1] === "auto";
          return {
            exitCode: passes ? 0 : 1,
            envelope: {
              data: {
                outcome: passes ? "passed" : "failed",
                passed: passes ? 1 : 0,
                failed: passes ? 0 : 1,
                browserParity: assisted ? "assisted" : "unmodified",
                corrections: [
                  { kind: "host-lan-asset-mirror", applied: assisted },
                ],
                environment: { comparable: true, preparationId },
              },
            },
          };
        };
        if (passes) await runParityGate({ run });
        else
          await assert.rejects(
            runParityGate({ run }),
            /unmodified-browser Studio F5 path failed/,
          );
        assert.equal(
          calls.some(
            (args) =>
              args[0] === "runtime" && ["start", "stop"].includes(args[1]),
          ),
          false,
        );
        assert.equal(
          calls.filter((args) => args[1] === "test").length,
          passes ? 2 : 1,
        );
        const report = JSON.parse(
          await fs.readFile(settings.MENDIMARU_STUDIO_PARITY_EVIDENCE, "utf8"),
        );
        assert.equal(report.outcome, passes ? "passed" : "failed");
        assert.equal(report.preparationId, preparationId);
      }
    } finally {
      for (const [key, value] of Object.entries(previous)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      await fs.rm(temporary, { recursive: true, force: true });
    }
  },
);

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
