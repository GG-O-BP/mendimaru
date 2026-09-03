import { describe, expect, it } from "vitest";
import type { Translate } from "../../i18n";
import { diagnosticText } from "./environmentDiagnostics";

const t: Translate = (key) => key;

describe("environment diagnostic process failures", () => {
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
});
