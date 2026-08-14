import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useFeedback } from "./useFeedback";

describe("useFeedback", () => {
  it("closes a confirmation before running its action", () => {
    const action = vi.fn(async () => undefined);
    const { result } = renderHook(() => useFeedback());

    act(() => {
      result.current.requestConfirmation({
        title: "Remove",
        description: "Remove this version?",
        confirmLabel: "Remove",
        action,
      });
    });
    act(() => result.current.acceptConfirmation());

    expect(result.current.confirmation).toBeNull();
    expect(action).toHaveBeenCalledOnce();
  });
});
