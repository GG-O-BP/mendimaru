import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConfigurationRecovery } from "./Feedback";

const translate = (key: string) => key;

describe("ConfigurationRecovery", () => {
  it("shows the load failure and exposes an explicit recovery action", () => {
    const onRecover = vi.fn();
    render(
      <ConfigurationRecovery
        t={translate}
        detail="Invalid config.json"
        busy={false}
        onRecover={onRecover}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "config-recovery-title" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Invalid config.json")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "action-recover-settings" }),
    );
    expect(onRecover).toHaveBeenCalledOnce();
  });
});
