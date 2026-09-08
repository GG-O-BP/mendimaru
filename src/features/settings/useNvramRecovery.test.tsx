import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ConfirmationState,
  NvramRecoveryPreview,
} from "../../domain/types";
import { useNvramRecovery } from "./useNvramRecovery";

const api = vi.hoisted(() => ({
  previewWinBoatNvram: vi.fn(),
  recoverWinBoatNvram: vi.fn(),
  restoreWinBoatNvram: vi.fn(),
}));
vi.mock("../../api/tauri", () => ({ tauriApi: api }));
const preview: NvramRecoveryPreview = {
  id: "exact-preview",
  targetPath: "/private/storage/windows.vars",
  backupPath: "/private/storage/backup.bak",
  originalPath: "/private/storage/retired.original",
  bytes: 540672,
  sha256: "a".repeat(64),
  evidence: "erased-ovmf-variable-store",
};
function setup() {
  const requestConfirmation = vi.fn<(state: ConfirmationState) => void>();
  const notify = vi.fn();
  const refresh = vi.fn().mockResolvedValue(undefined);
  const t = (key: string, values?: Record<string, string | number>) =>
    `${key} ${JSON.stringify(values ?? {})}`;
  const dependencies = {
    requestConfirmation,
    notify,
    t,
    runAction: async (_key: string, action: () => Promise<void>) => action(),
    isBusy: () => false,
    onWarning: vi.fn(),
  };
  return {
    ...renderHook(() => useNvramRecovery(dependencies, refresh)),
    requestConfirmation,
    notify,
    refresh,
  };
}

describe("explicit UEFI recovery confirmation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.previewWinBoatNvram.mockResolvedValue(preview);
  });
  it("requires approval of the exact preview before invoking recovery", async () => {
    const { result, requestConfirmation } = setup();
    await act(() => result.current.recoverNvram());
    expect(api.recoverWinBoatNvram).not.toHaveBeenCalled();
    const confirmation = requestConfirmation.mock.calls[0][0];
    expect(confirmation.description).toContain(preview.targetPath);
    expect(confirmation.description).toContain(preview.backupPath);
    expect(confirmation.description).toContain(preview.sha256);
    api.recoverWinBoatNvram.mockResolvedValue({
      ready: true,
      rolledBack: false,
      rollbackRequired: false,
    });
    await act(() => confirmation.action());
    expect(api.recoverWinBoatNvram).toHaveBeenCalledWith(preview.id);
  });

  it("retains a confirmed restoration action when automatic rollback is unavailable", async () => {
    const { result, requestConfirmation } = setup();
    api.recoverWinBoatNvram.mockResolvedValue({
      ready: false,
      rolledBack: false,
      rollbackRequired: true,
    });
    await act(() => result.current.recoverNvram());
    await act(() => requestConfirmation.mock.calls[0][0].action());
    expect(result.current.rollbackRequired).toBe(true);
    act(() => result.current.restoreNvram());
    expect(api.restoreWinBoatNvram).not.toHaveBeenCalled();
    api.restoreWinBoatNvram.mockResolvedValue({
      ready: false,
      rolledBack: true,
      rollbackRequired: false,
    });
    await act(() => requestConfirmation.mock.calls[1][0].action());
    expect(api.restoreWinBoatNvram).toHaveBeenCalledWith(preview.id);
    expect(result.current.rollbackRequired).toBe(false);
  });
});
