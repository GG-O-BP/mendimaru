import assert from "node:assert/strict";
import test from "node:test";
import {
  collectRustsecWarnings,
  verifyRustsecPolicy,
} from "./verify-rustsec-policy.mjs";

const now = new Date("2026-09-02T00:00:00.000Z");

function advisoryWarning(kind, id, name, version) {
  return {
    kind,
    package: { name, version },
    advisory: { id },
  };
}

function report({ warnings = {}, vulnerabilities = [] } = {}) {
  return {
    warnings,
    vulnerabilities: {
      count: vulnerabilities.length,
      list: vulnerabilities,
    },
  };
}

function maintainedException(overrides = {}) {
  return {
    advisoryId: "RUSTSEC-2026-0001",
    kind: "unmaintained",
    package: { name: "fixture", version: "1.0.0" },
    owner: "GGOBP",
    introducedBy: ["fixture-introducer"],
    reason: "Retained while the upstream desktop stack migrates.",
    upstreamUrl: "https://example.com/advisory",
    remediationPlan: "Upgrade the desktop stack and remove the exception.",
    expiresAt: "2026-12-31",
    ...overrides,
  };
}

test("collects advisory and yanked warnings into stable keys", () => {
  const warnings = collectRustsecWarnings(
    report({
      warnings: {
        unmaintained: [
          advisoryWarning("unmaintained", "RUSTSEC-2026-0001", "old", "1.0.0"),
        ],
        unsound: [
          advisoryWarning("unsound", "RUSTSEC-2026-0002", "unsound", "2.0.0"),
        ],
        yanked: [{ package: { name: "yanked", version: "3.0.0" } }],
      },
    }),
  );

  assert.deepEqual(
    warnings.map((warning) => warning.key),
    [
      "unmaintained:RUSTSEC-2026-0001",
      "unsound:RUSTSEC-2026-0002",
      "yanked:yanked@3.0.0",
    ],
  );
});

test("accepts an owned, current, exact unmaintained exception", () => {
  const result = verifyRustsecPolicy(
    report({
      warnings: {
        unmaintained: [
          advisoryWarning(
            "unmaintained",
            "RUSTSEC-2026-0001",
            "fixture",
            "1.0.0",
          ),
        ],
      },
    }),
    { exceptions: [maintainedException()] },
    now,
  );
  assert.equal(result.ok, true);
  assert.deepEqual(result.errors, []);
});

test("denies vulnerabilities, unknown warnings, yanks, and stale exceptions", () => {
  const result = verifyRustsecPolicy(
    report({
      warnings: {
        unmaintained: [
          advisoryWarning(
            "unmaintained",
            "RUSTSEC-2026-9999",
            "unknown",
            "1.0.0",
          ),
        ],
        yanked: [{ package: { name: "yanked", version: "3.0.0" } }],
      },
      vulnerabilities: [
        advisoryWarning(
          "vulnerability",
          "RUSTSEC-2026-9998",
          "vulnerable",
          "4.0.0",
        ),
      ],
    }),
    { exceptions: [maintainedException()] },
    now,
  );

  assert.equal(result.ok, false);
  assert.match(result.errors.join("\n"), /vulnerability denied/);
  assert.match(result.errors.join("\n"), /unknown RustSec warning denied/);
  assert.match(result.errors.join("\n"), /yanked dependency denied/);
  assert.match(result.errors.join("\n"), /stale policy exception/);
});

test("expired and package-drifted exceptions fail", () => {
  const result = verifyRustsecPolicy(
    report({
      warnings: {
        unmaintained: [
          advisoryWarning(
            "unmaintained",
            "RUSTSEC-2026-0001",
            "fixture",
            "1.1.0",
          ),
        ],
      },
    }),
    {
      exceptions: [maintainedException({ expiresAt: "2026-09-01" })],
    },
    now,
  );

  assert.equal(result.ok, false);
  assert.match(result.errors.join("\n"), /expired on 2026-09-01/);
  assert.match(result.errors.join("\n"), /lockfile has fixture@1\.1\.0/);
});

test("unsound warnings require bounded non-reachability evidence", () => {
  const warning = advisoryWarning(
    "unsound",
    "RUSTSEC-2026-0002",
    "unsound",
    "2.0.0",
  );
  const policy = {
    exceptions: [
      maintainedException({
        advisoryId: "RUSTSEC-2026-0002",
        kind: "unsound",
        package: { name: "unsound", version: "2.0.0" },
      }),
    ],
  };
  const denied = verifyRustsecPolicy(
    report({ warnings: { unsound: [warning] } }),
    policy,
    now,
  );
  assert.equal(denied.ok, false);
  assert.match(denied.errors.join("\n"), /unsupported unsound exception class/);
  assert.match(
    denied.errors.join("\n"),
    /lacks complete non-reachability evidence/,
  );

  const accepted = verifyRustsecPolicy(
    report({ warnings: { unsound: [warning] } }),
    {
      exceptions: [
        maintainedException({
          advisoryId: "RUSTSEC-2026-0002",
          kind: "unsound",
          exceptionClass: "bounded-reachability",
          package: { name: "unsound", version: "2.0.0" },
          reachability: {
            status: "not-reachable",
            evidence: "No source reference to the affected API.",
            checkedCommit: "0123456789abcdef0123456789abcdef01234567",
          },
        }),
      ],
    },
    now,
  );
  assert.equal(accepted.ok, true);
});
