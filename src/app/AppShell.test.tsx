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

function shell(online: boolean) {
  return (
    <AppShell
      t={createTranslate(localization)}
      localization={localization}
      activeView="studio"
      online={online}
      warning={null}
      languageChanging={false}
      winBoatControl={{
        kind: "open",
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
    expect(route?.querySelector(".route-track i")).not.toBeInTheDocument();

    view.rerender(shell(true));
    expect(route).toHaveClass("online");
    expect(route?.querySelector(".route-track i")).toBeInTheDocument();

    view.rerender(shell(false));
    expect(route).toHaveClass("offline");
    expect(route?.querySelector(".route-track i")).not.toBeInTheDocument();
  });
});
