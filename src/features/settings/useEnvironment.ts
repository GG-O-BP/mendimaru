import type { EnvironmentDependencies } from "./dependencies";
import { useEnvironmentStatus } from "./useEnvironmentStatus";
import { useSettingsDraft } from "./useSettingsDraft";
import { useWinBoatControl } from "./useWinBoatControl";

export function useEnvironment(dependencies: EnvironmentDependencies) {
  const environmentStatus = useEnvironmentStatus(dependencies);
  const settings = useSettingsDraft({
    ...dependencies,
    environmentStatus: environmentStatus.status,
    refreshStatus: environmentStatus.refreshStatus,
  });
  const winBoat = useWinBoatControl({
    ...dependencies,
    status: environmentStatus.status,
    refreshStatus: environmentStatus.refreshStatus,
    applyConfig: settings.applyConfig,
    updateConfigPair: settings.updateConfigPair,
  });

  return {
    config: settings.config,
    draftConfig: settings.draftConfig,
    setDraftConfig: settings.setDraftConfig,
    status: environmentStatus.status,
    online: winBoat.online,
    loading: settings.loading,
    applyMountNow: settings.applyMountNow,
    setApplyMountNow: settings.setApplyMountNow,
    settingsChanged: settings.settingsChanged,
    refreshStatus: environmentStatus.refreshStatus,
    choosePath: settings.choosePath,
    saveSettings: settings.saveSettings,
    redetectSettings: settings.redetectSettings,
    updateLanguagePreference: settings.updateLanguagePreference,
    winBoatControl: winBoat.winBoatControl,
    offlineGuidance: winBoat.offlineGuidance,
    setupCompletion: winBoat.setupCompletion,
  };
}

export type EnvironmentController = ReturnType<typeof useEnvironment>;
