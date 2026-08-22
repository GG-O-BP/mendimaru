import type { ConfirmationState, ToastKind } from "../../domain/types";
import type { Translate } from "../../i18n";

export interface StudioDependencies {
  t: Translate;
  installedVersionsSourceKey: string;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
  requestConfirmation: (state: ConfirmationState) => void;
  runAction: (key: string, action: () => Promise<void>) => Promise<void>;
  isBusy: (key: string) => boolean;
  hasBusyPrefix: (prefix: string) => boolean;
  onWarning: (message: string | null) => void;
}

export type InstalledVersionsDependencies = Pick<
  StudioDependencies,
  | "t"
  | "installedVersionsSourceKey"
  | "notify"
  | "requestConfirmation"
  | "runAction"
  | "hasBusyPrefix"
  | "onWarning"
>;

export type VersionCatalogDependencies = Pick<StudioDependencies, "t">;

export type StudioInstallationDependencies = Pick<
  StudioDependencies,
  | "t"
  | "notify"
  | "requestConfirmation"
  | "runAction"
  | "hasBusyPrefix"
  | "onWarning"
>;
