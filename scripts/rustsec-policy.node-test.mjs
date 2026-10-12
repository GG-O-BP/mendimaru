import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  collectRustsecWarnings,
  EXPIRY_WARNING_DAYS,
  formatExpiryWarning,
  isValidIsoDate,
  verifyExceptionExpiry,
  verifyRustsecPolicy,
} from "./verify-rustsec-policy.mjs";

const script = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "verify-rustsec-policy.mjs",
);

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

test("rejects impossible calendar dates instead of comparing strings", () => {
  assert.equal(isValidIsoDate("2026-12-31"), true);
  assert.equal(isValidIsoDate("2028-02-29"), true);
  for (const value of [
    "2026-02-30",
    "2026-13-01",
    "2026-1-01",
    "",
    null,
    undefined,
    20261231,
  ]) {
    assert.equal(isValidIsoDate(value), false, String(value));
  }

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
    { exceptions: [maintainedException({ expiresAt: "2026-13-01" })] },
    now,
  );
  assert.equal(result.ok, false);
  assert.match(result.errors.join("\n"), /invalid expiresAt date/);
});

test("expiry-only verification needs no audit report and fails closed", () => {
  const current = verifyExceptionExpiry(
    { exceptions: [maintainedException({ expiresAt: "2026-12-31" })] },
    now,
  );
  assert.equal(current.ok, true);
  assert.deepEqual(current.errors, []);
  assert.deepEqual(current.expiringSoon, []);

  // The exact failure of #228: an unchanged manifest whose date has passed.
  const expired = verifyExceptionExpiry(
    {
      exceptions: [
        maintainedException(),
        maintainedException({
          advisoryId: "RUSTSEC-2024-0429",
          kind: "unsound",
          expiresAt: "2026-10-01",
        }),
      ],
    },
    new Date("2026-10-12T05:00:00.000Z"),
  );
  assert.equal(expired.ok, false);
  assert.deepEqual(expired.errors, [
    "policy exception expired on 2026-10-01: unsound:RUSTSEC-2024-0429",
  ]);

  for (const policy of [{}, { exceptions: "none" }, null]) {
    const malformed = verifyExceptionExpiry(policy, now);
    assert.equal(malformed.ok, false);
    assert.match(malformed.errors.join("\n"), /no exceptions list/);
  }

  const invalid = verifyExceptionExpiry(
    { exceptions: [maintainedException({ expiresAt: "2026-02-30" })] },
    now,
  );
  assert.equal(invalid.ok, false);
  assert.match(invalid.errors.join("\n"), /invalid expiresAt date/);

  assert.equal(verifyExceptionExpiry({ exceptions: [] }, now).ok, true);
});

test("the last valid day passes and upcoming expiries are warned in advance", () => {
  const lastDay = verifyExceptionExpiry(
    { exceptions: [maintainedException({ expiresAt: "2026-09-02" })] },
    new Date("2026-09-02T23:59:59.000Z"),
  );
  assert.equal(lastDay.ok, true);
  assert.deepEqual(lastDay.expiringSoon, [
    {
      key: "unmaintained:RUSTSEC-2026-0001",
      expiresAt: "2026-09-02",
      daysRemaining: 0,
    },
  ]);

  const dayAfter = verifyExceptionExpiry(
    { exceptions: [maintainedException({ expiresAt: "2026-09-02" })] },
    new Date("2026-09-03T00:00:00.000Z"),
  );
  assert.equal(dayAfter.ok, false);

  const window = verifyExceptionExpiry(
    {
      exceptions: [
        maintainedException({ expiresAt: "2026-10-02" }),
        maintainedException({
          advisoryId: "RUSTSEC-2026-0003",
          expiresAt: "2026-10-03",
        }),
      ],
    },
    now,
  );
  assert.equal(EXPIRY_WARNING_DAYS, 30);
  assert.equal(window.ok, true);
  assert.deepEqual(
    window.expiringSoon.map((entry) => [entry.key, entry.daysRemaining]),
    [["unmaintained:RUSTSEC-2026-0001", 30]],
  );

  assert.equal(
    formatExpiryWarning(window.expiringSoon[0], true),
    "::warning title=RustSec exception expiring::RustSec policy exception unmaintained:RUSTSEC-2026-0001 expires on 2026-10-02 (30 day(s) left); re-review it before then",
  );
  assert.match(
    formatExpiryWarning(window.expiringSoon[0], false),
    /^warning: RustSec policy exception/,
  );
});

test("the full audit verdict also reports active exceptions about to expire", () => {
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
    { exceptions: [maintainedException({ expiresAt: "2026-09-10" })] },
    now,
  );
  assert.equal(result.ok, true);
  assert.deepEqual(result.expiringSoon, [
    {
      key: "unmaintained:RUSTSEC-2026-0001",
      expiresAt: "2026-09-10",
      daysRemaining: 8,
    },
  ]);
});

function runExpiryCli(policy, ...extra) {
  return runExpiryCliWithEnv({}, policy, ...extra);
}

function runExpiryCliWithEnv(environment, policy, ...extra) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "rustsec-expiry-"));
  try {
    const policyPath = path.join(directory, "policy.json");
    fs.writeFileSync(policyPath, JSON.stringify(policy));
    return spawnSync(
      process.execPath,
      [script, "--expiry-only", "--policy", policyPath, ...extra],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          GITHUB_ACTIONS: "true",
          RUSTSEC_WARNING_FORMAT: "",
          ...environment,
        },
      },
    );
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test("the expiry-only CLI gates on the calendar without a cargo audit report", () => {
  const passing = runExpiryCli(
    { exceptions: [maintainedException({ expiresAt: "2026-12-31" })] },
    "--today",
    "2026-12-10",
  );
  assert.equal(passing.status, 0, passing.stderr);
  assert.match(passing.stdout, /RustSec exception expiry: PASS/);
  assert.match(
    passing.stdout,
    /::warning title=RustSec exception expiring::.*2026-12-31 \(21 day\(s\) left\)/,
  );

  const failing = runExpiryCli(
    { exceptions: [maintainedException({ expiresAt: "2026-10-01" })] },
    "--today",
    "2026-10-12",
  );
  assert.equal(failing.status, 1);
  assert.match(failing.stdout, /RustSec exception expiry: FAIL/);
  assert.match(
    failing.stderr,
    /policy exception expired on 2026-10-01: unmaintained:RUSTSEC-2026-0001/,
  );

  const quiet = runExpiryCli(
    { exceptions: [maintainedException({ expiresAt: "2026-12-31" })] },
    "--today",
    "2026-12-10",
    "--warning-days",
    "7",
  );
  assert.equal(quiet.status, 0, quiet.stderr);
  assert.doesNotMatch(quiet.stdout, /::warning/);

  const badDate = runExpiryCli({ exceptions: [] }, "--today", "2026-02-30");
  assert.notEqual(badDate.status, 0);
  assert.match(badDate.stderr, /--today requires a YYYY-MM-DD date/);
});

test("the Markdown summary can request plain expiry warnings", () => {
  const plain = runExpiryCliWithEnv(
    { RUSTSEC_WARNING_FORMAT: "plain" },
    { exceptions: [maintainedException({ expiresAt: "2026-12-31" })] },
    "--today",
    "2026-12-30",
  );
  assert.equal(plain.status, 0, plain.stderr);
  assert.doesNotMatch(plain.stdout, /::warning/);
  assert.match(
    plain.stdout,
    /^warning: RustSec policy exception .*2026-12-31/m,
  );
});
