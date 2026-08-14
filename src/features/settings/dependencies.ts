import type { ConfirmationState, ToastKind } from "../../domain/types";
import type { Translate } from "../../i18n";

export interface EnvironmentDependencies {
  t: Translate;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
  requestConfirmation: (state: ConfirmationState) => void;
  runAction: (key: string, action: () => Promise<void>) => Promise<void>;
  isBusy: (key: string) => boolean;
  onWarning: (message: string | null) => void;
}

export type EnvironmentStatusDependencies = Pick<
  EnvironmentDependencies,
  "t" | "onWarning"
>;

export type SettingsDraftDependencies = Pick<
  EnvironmentDependencies,
  "t" | "notify" | "requestConfirmation" | "runAction" | "onWarning"
>;

export type WinBoatControlDependencies = Pick<
  EnvironmentDependencies,
  "t" | "notify" | "runAction" | "isBusy"
>;
