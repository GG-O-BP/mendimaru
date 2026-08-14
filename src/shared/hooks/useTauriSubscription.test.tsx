import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useTauriSubscription } from "./useTauriSubscription";

describe("useTauriSubscription", () => {
  it("unsubscribes when registration resolves after unmount", async () => {
    const unsubscribe = vi.fn();
    let resolveSubscription!: (value: () => void) => void;
    const subscribe = vi.fn(
      () =>
        new Promise<() => void>((resolve) => {
          resolveSubscription = resolve;
        }),
    );
    const { unmount } = renderHook(() => useTauriSubscription(subscribe));

    await act(async () => {
      await Promise.resolve();
    });
    unmount();
    await act(async () => {
      resolveSubscription(unsubscribe);
      await Promise.resolve();
    });

    expect(unsubscribe).toHaveBeenCalledOnce();
  });

  it("unsubscribes an active registration on unmount", async () => {
    const unsubscribe = vi.fn();
    const subscribe = vi.fn().mockResolvedValue(unsubscribe);
    const { unmount } = renderHook(() => useTauriSubscription(subscribe));

    await act(async () => {
      await Promise.resolve();
    });
    unmount();

    expect(unsubscribe).toHaveBeenCalledOnce();
  });

  it("reports registration failures only while mounted", async () => {
    const error = new Error("registration failed");
    const onError = vi.fn();
    const subscribe = vi.fn().mockRejectedValue(error);
    renderHook(() => useTauriSubscription(subscribe, onError));

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(onError).toHaveBeenCalledWith(error);
  });
});
