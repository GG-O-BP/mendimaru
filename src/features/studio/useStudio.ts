import type { StudioDependencies } from "./dependencies";
import { useInstalledVersions } from "./useInstalledVersions";
import { useInstallQueue } from "./useInstallQueue";
import { useStudioInstallation } from "./useStudioInstallation";
import { useVersionCatalog } from "./useVersionCatalog";

export function useStudio(dependencies: StudioDependencies) {
  const installedVersions = useInstalledVersions(dependencies);
  const catalog = useVersionCatalog(dependencies);
  const installation = useStudioInstallation({
    ...dependencies,
    refreshInstalled: installedVersions.refreshInstalled,
  });
  const installQueue = useInstallQueue({
    ...dependencies,
    refreshInstalled: installedVersions.refreshInstalled,
  });

  return {
    ...installedVersions,
    ...catalog,
    ...installation,
    installQueue,
    isBusy: dependencies.isBusy,
  };
}
