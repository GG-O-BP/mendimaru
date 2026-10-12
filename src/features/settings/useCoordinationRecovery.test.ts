import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ConfirmationState } from "../../domain/types";
import type { Translate } from "../../i18n";
import { useCoordinationRecovery } from "./useCoordinationRecovery";
const api = vi.hoisted(() => ({
  previewCoordinationPermissions: vi.fn(),
  repairCoordinationPermissions: vi.fn(),
}));
vi.mock("../../api/tauri", () => ({ tauriApi: api }));

describe("coordination permission recovery", () => {
  it("shows the exact reviewed entries and performs no repair until confirmed", async () => {
    api.previewCoordinationPermissions.mockResolvedValue({
      id: "preview",
      directory: "/tmp/private-fixture",
      files: ["/tmp/private-fixture/entry.lock"],
    });
    api.repairCoordinationPermissions.mockResolvedValue(undefined);
    const confirm = vi.fn<(state: ConfirmationState) => void>();
    const refresh = vi.fn(async () => {});
    const notify = vi.fn();
    const { result } = renderHook(() =>
      useCoordinationRecovery(
        {
          t: ((key, args) =>
            `${key} ${JSON.stringify(args ?? {})}`) as Translate,
          requestConfirmation: confirm,
          notify,
          runAction: async (_key, action) => action(),
          isBusy: () => false,
          onWarning: () => {},
        },
        refresh,
      ),
    );
    await act(async () => {
      await result.current.previewRepair();
    });
    expect(api.repairCoordinationPermissions).not.toHaveBeenCalled();
    const prompt = confirm.mock.calls[0][0];
    expect(prompt.description).toContain("/tmp/private-fixture/entry.lock");
    expect(prompt.danger).toBe(true);
    await act(async () => {
      await prompt.action();
    });
    expect(api.repairCoordinationPermissions).toHaveBeenCalledWith("preview");
    expect(refresh).toHaveBeenCalledOnce();
    expect(notify).toHaveBeenCalledOnce();
  });
});
