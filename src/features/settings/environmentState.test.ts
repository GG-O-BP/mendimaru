import { describe, expect, it } from "vitest";
import type { EnvironmentStatus } from "../../domain/types";
import { deriveEnvironmentPresentation } from "./environmentState";

const translate = (key: string) => key;

function status(overrides: Partial<EnvironmentStatus> = {}): EnvironmentStatus {
  return {
    winboatAvailable: true,
    winboatInitialized: true,
    setupPending: false,
    composeAvailable: true,
    runtimeAvailable: true,
    freerdpAvailable: true,
    sharedDirectoryAvailable: true,
    sharedMountMatches: true,
    containerStatus: "exited",
    guestOnline: false,
    ...overrides,
  };
}

describe("environment presentation", () => {
  it("routes a missing WinBoat installation to settings", () => {
    const presentation = deriveEnvironmentPresentation(null, translate);

    expect(presentation.controlKind).toBe("settings");
    expect(presentation.actionKey).toBe("winboat-settings");
    expect(presentation.offlineGuidance.title).toBe("winboat-missing-title");
  });

  it("distinguishes setup, startup, and running states", () => {
    const setup = deriveEnvironmentPresentation(
      status({ winboatInitialized: false, setupPending: true }),
      translate,
    );
    expect(setup.controlKind).toBe("setup");
    expect(setup.actionLabel).toBe("action-continue-winboat-setup");

    const stopped = deriveEnvironmentPresentation(status(), translate);
    expect(stopped.controlKind).toBe("start");
    expect(stopped.offlineGuidance.title).toBe("offline-guidance-title");

    const running = deriveEnvironmentPresentation(
      status({ containerStatus: "running" }),
      translate,
    );
    expect(running.controlKind).toBe("open");
    expect(running.offlineGuidance.title).toBe("windows-preparing-title");
  });

  it("marks the guest as online only from the backend health result", () => {
    const presentation = deriveEnvironmentPresentation(
      status({ containerStatus: "running", guestOnline: true }),
      translate,
    );

    expect(presentation.online).toBe(true);
    expect(presentation.actionLabel).toBe("action-open-winboat");
  });
});
