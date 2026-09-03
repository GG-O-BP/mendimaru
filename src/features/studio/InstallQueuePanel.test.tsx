import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { InstallQueueItem, LocalizationBundle } from "../../domain/types";
import type { Translate } from "../../i18n";
import { InstallQueuePanel } from "./InstallQueuePanel";

const localization: LocalizationBundle = {
  locale: "en-US",
  preference: "system",
  direction: "ltr",
  availableLocales: [{ id: "en-US", nativeName: "English" }],
  messages: {},
  numbers: [],
};

const t: Translate = (key, values) =>
  values ? `${key}:${values.version ?? ""}` : key;

function item(overrides: Partial<InstallQueueItem>): InstallQueueItem {
  return {
    id: "item-1",
    version: "11.12.2",
    forceRedownload: false,
    state: "queued",
    createdAt: "2026-09-03T00:00:00Z",
    updatedAt: "2026-09-03T00:00:00Z",
    ...overrides,
  };
}

function renderPanel(items: InstallQueueItem[]) {
  const handlers = {
    onCancel: vi.fn(),
    onDiscard: vi.fn(),
    onRetry: vi.fn(),
    onMove: vi.fn(),
    onRemove: vi.fn(),
  };
  render(
    <InstallQueuePanel
      t={t}
      localization={localization}
      items={items}
      {...handlers}
    />,
  );
  return handlers;
}

describe("InstallQueuePanel", () => {
  it("hides itself when the queue is empty", () => {
    renderPanel([]);
    expect(screen.queryByTestId("install-queue")).toBeNull();
  });

  it("offers ordering and cancellation for pending items", async () => {
    const handlers = renderPanel([
      item({ id: "first", version: "11.12.2" }),
      item({ id: "second", version: "11.13.0" }),
    ]);

    expect(screen.getAllByText("install-queue-state-queued")).toHaveLength(2);
    fireEvent.click(
      screen.getByRole("button", { name: "install-queue-move-up:11.13.0" }),
    );
    expect(handlers.onMove).toHaveBeenCalledWith("second", true);

    fireEvent.click(screen.getAllByText("install-queue-cancel-keep")[0]);
    expect(handlers.onCancel).toHaveBeenCalledWith("first");
    fireEvent.click(screen.getAllByText("install-queue-cancel-discard")[0]);
    expect(handlers.onDiscard).toHaveBeenCalledWith("first");
  });

  it("shows active progress and retry or removal for terminal items", async () => {
    const handlers = renderPanel([
      item({
        id: "active",
        state: "downloading",
        downloadedBytes: 10,
        totalBytes: 100,
        percentage: 10,
      }),
      item({
        id: "failed",
        state: "failed",
        message: "installer failed",
      }),
      item({
        id: "done",
        state: "succeeded",
        version: "11.14.0",
        message: "operation-7",
      }),
    ]);

    expect(screen.getByRole("progressbar")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toBe("installer failed");

    fireEvent.click(screen.getByText("action-retry"));
    expect(handlers.onRetry).toHaveBeenCalledWith("failed");

    fireEvent.click(
      screen.getByRole("button", { name: "install-queue-remove:11.12.2" }),
    );
    expect(handlers.onRemove).toHaveBeenCalledWith("failed");
    fireEvent.click(
      screen.getByRole("button", { name: "install-queue-remove:11.14.0" }),
    );
    expect(handlers.onRemove).toHaveBeenCalledWith("done");
  });
});
