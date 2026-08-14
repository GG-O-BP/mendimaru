import type { EnvironmentController } from "../features/settings/useEnvironment";
import { SettingsPage } from "../features/settings/SettingsPage";
import type { Translate } from "../i18n";

export function SettingsView({
  t,
  environment,
  isBusy,
}: {
  t: Translate;
  environment: EnvironmentController;
  isBusy: (key: string) => boolean;
}) {
  if (!environment.draftConfig) return null;

  return (
    <SettingsPage
      t={t}
      model={{
        config: environment.draftConfig,
        nativeWindows: environment.status?.platform.kind === "windows-native",
        changed: environment.settingsChanged,
        mountMatches: Boolean(environment.status?.sharedMountMatches),
        applyNow: environment.applyMountNow,
        diagnostics: environment.status?.diagnostics ?? [],
        isBusy,
        onChange: environment.setDraftConfig,
        onChoose: (field) => void environment.choosePath(field),
        onAddStudioPath: () => void environment.addStudioPath(),
        onRemoveStudioPath: environment.removeStudioPath,
        onApplyNow: environment.setApplyMountNow,
        onSave: environment.saveSettings,
        onRedetect: () => void environment.redetectSettings(),
        onDiagnosticAction: (action, id) =>
          void environment.runDiagnosticAction(action, id),
        onCopyDiagnosticReport: () => void environment.copyDiagnosticReport(),
        onExportDiagnosticReport: () =>
          void environment.exportDiagnosticReport(),
      }}
    />
  );
}
