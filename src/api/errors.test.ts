import { describe, expect, it } from "vitest";
import { errorCode, errorText, isCommandError } from "./errors";

const translate = (key: string) => `translated:${key}`;

describe("command errors", () => {
  it("recognizes the structured backend contract", () => {
    const error = {
      code: "download_cancelled",
      message: "Cancelled by the user",
    };

    expect(isCommandError(error)).toBe(true);
    expect(errorCode(error)).toBe("download_cancelled");
    expect(errorText(error, translate)).toBe("Cancelled by the user");
  });

  it("rejects unknown codes and malformed payloads", () => {
    expect(isCommandError({ code: "other", message: "Failure" })).toBe(false);
    expect(isCommandError({ code: "install_failed" })).toBe(false);
  });

  it("falls back safely for values that cannot be serialized", () => {
    const circular: Record<string, unknown> = {};
    circular.self = circular;

    expect(errorText(circular, translate)).toBe("translated:unknown-error");
  });
});
