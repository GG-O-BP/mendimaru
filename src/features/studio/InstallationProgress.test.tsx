import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type {
  DownloadProgress,
  DownloadState,
  LocalizationBundle,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import { InstallationProgress } from "./InstallationProgress";

const localization: LocalizationBundle = {
  locale: "en-US",
  preference: "system",
  direction: "ltr",
  availableLocales: [{ id: "en-US", nativeName: "English" }],
  messages: {},
  numbers: [],
};
const t: Translate = (key) => key;

function progress(state: DownloadState): DownloadProgress {
  return {
    version: "11.13.0",
    state,
    downloadedBytes: 50,
    totalBytes: 100,
    percentage: state === "installed" ? 100 : 50,
    estimated: false,
    message: state,
  };
}

describe("InstallationProgress motion state", () => {
  it("animates only while work is active and stops for every terminal state", () => {
    const view = render(
      <InstallationProgress
        t={t}
        localization={localization}
        progress={progress("downloading")}
        isInstalling
        onCancel={() => undefined}
      />,
    );

    expect(view.container.querySelector(".download-bar")).toHaveAttribute(
      "aria-busy",
      "true",
    );
    expect(view.container.querySelector(".progress-track > span")).toHaveClass(
      "active",
    );

    for (const state of ["installed", "failed", "cancelled"] as const) {
      view.rerender(
        <InstallationProgress
          t={t}
          localization={localization}
          progress={progress(state)}
          isInstalling={false}
          onCancel={() => undefined}
        />,
      );
      expect(view.container.querySelector(".download-bar")).toHaveAttribute(
        "aria-busy",
        "false",
      );
      expect(
        view.container.querySelector(".progress-track > span"),
      ).not.toHaveClass("active");
    }
  });
});
