import { describe, expect, it } from "vitest";
import type { Translate } from "../../i18n";
import { diagnosticText } from "./environmentDiagnostics";

const t: Translate = (key) => key;

describe("environment diagnostic process failures", () => {
  it("renders QEMU's stable diagnosis without remote output", () => {
    const text = diagnosticText(
      {
        id: "container",
        status: "failure",
        action: "open-winboat",
        errorCode: "qemu-boot-timeout",
        observed: "password=secret /home/private",
      },
      t,
    );
    expect(text.detail).toBe("diagnostic-qemu-boot-timeout");
    expect(text.action).toBe("diagnostic-action-open-winboat");
    expect(JSON.stringify(text)).not.toContain("secret");
  });
  it("renders a stable timeout recovery message instead of a generic probe failure", () => {
    const text = diagnosticText(
      {
        id: "container-runtime",
        status: "failure",
        action: "open-settings",
        errorCode: "external-process-timeout",
      },
      t,
    );

    expect(text.detail).toBe("diagnostic-process-timeout");
    expect(text.action).toBe("diagnostic-action-open-settings");
  });

  it("renders the stable guest clock skew recovery message", () => {
    const text = diagnosticText(
      {
        id: "guest-clock",
        status: "failure",
        action: "redetect",
        errorCode: "guest-clock-skew-exceeded",
      },
      t,
    );

    expect(text.title).toBe("diagnostic-guest-clock-title");
    expect(text.detail).toBe("diagnostic-clock-skew-exceeded");
    expect(text.action).toBe("diagnostic-action-redetect");
  });

  it("renders each coordination lock cause with its own recovery message", () => {
    for (const [errorCode, detail, action] of [
      [
        "coordination-lock-untrusted",
        "diagnostic-coordination-lock-untrusted",
        "redetect",
      ],
      [
        "coordination-lock-unavailable",
        "diagnostic-coordination-lock-unavailable",
        "redetect",
      ],
      [
        "coordination-identity-mismatch",
        "diagnostic-coordination-identity-mismatch",
        "open-settings",
      ],
    ] as const) {
      const text = diagnosticText(
        { id: "coordination-lock", status: "failure", action, errorCode },
        t,
      );
      expect(text.title).toBe("diagnostic-coordination-lock-title");
      expect(text.detail).toBe(detail);
      expect(text.detail).not.toBe("windows-startup-failed-detail");
    }
    expect(
      diagnosticText({ id: "coordination-lock", status: "success" }, t).detail,
    ).toBe("diagnostic-check-passed");
  });
});
