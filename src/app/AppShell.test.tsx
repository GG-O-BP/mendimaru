import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { LocalizationBundle } from "../domain/types";
import { createTranslate } from "../i18n";
import { AppShell } from "./AppShell";

const localization: LocalizationBundle = {
  locale: "en-US",
  preference: "system",
  direction: "ltr",
  availableLocales: [{ id: "en-US", nativeName: "English" }],
  messages: {},
  numbers: [],
};

function shell(online: boolean, nativeWindows = false) {
  return (
    <AppShell
      t={createTranslate(localization)}
      localization={localization}
      activeView="studio"
      online={online}
      warning={null}
      languageChanging={false}
      winBoatControl={{
        kind: nativeWindows ? "native" : "open",
        label: "Open WinBoat",
        busy: false,
        onAction: vi.fn(),
      }}
      onViewChange={vi.fn()}
      onLanguageChange={vi.fn()}
      onDismissWarning={vi.fn()}
    >
      <main>content</main>
    </AppShell>
  );
}

describe("AppShell route status", () => {
  it("mounts the moving marker only when the WinBoat route becomes online", () => {
    const view = render(shell(false));
    const route = view.container.querySelector(".route-status");
    expect(route).toHaveClass("offline");
    expect(route?.querySelector(".route-packet")).not.toBeInTheDocument();

    view.rerender(shell(true));
    expect(route).toHaveClass("online");
    expect(route?.querySelector(".route-packet")).toBeInTheDocument();

    view.rerender(shell(false));
    expect(route).toHaveClass("offline");
    expect(route?.querySelector(".route-packet")).not.toBeInTheDocument();
  });

  it("never renders Linux route motion for the native Windows header", () => {
    const view = render(shell(true, true));
    expect(view.container.querySelector(".route-status")).toHaveClass("online");
    expect(
      view.container.querySelector(".route-track"),
    ).not.toBeInTheDocument();
    expect(
      view.container.querySelector(".route-packet"),
    ).not.toBeInTheDocument();
  });
});
