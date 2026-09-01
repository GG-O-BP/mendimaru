import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { MendixProject } from "../../domain/types";
import type { Translate } from "../../i18n";

export function useProjects(
  t: Translate,
  onWarning: (message: string | null) => void,
  runAction: (key: string, action: () => Promise<void>) => Promise<void>,
) {
  const [projects, setProjects] = useState<MendixProject[]>([]);
  const [search, setSearch] = useState("");
  const [externalSelectionBusy, setExternalSelectionBusy] = useState(false);
  const externalSelectionLock = useRef(false);

  const refresh = useCallback(async () => {
    try {
      setProjects(await tauriApi.getProjects());
    } catch (error) {
      onWarning(errorText(error, t));
    }
  }, [onWarning, t]);

  useEffect(() => {
    const initialRefresh = window.setTimeout(() => void refresh(), 0);
    return () => window.clearTimeout(initialRefresh);
  }, [refresh]);

  const filteredProjects = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return needle
      ? projects.filter((project) =>
          `${project.name} ${project.directory} ${project.version ?? ""}`
            .toLowerCase()
            .includes(needle),
        )
      : projects;
  }, [projects, search]);

  const openFolder = useCallback(
    (path: string) =>
      runAction(`folder-${path}`, () => tauriApi.openFolder(path)),
    [runAction],
  );

  const selectExternalProject = useCallback(async () => {
    if (externalSelectionLock.current) return null;
    externalSelectionLock.current = true;
    setExternalSelectionBusy(true);
    try {
      return await tauriApi.selectExternalProject();
    } catch (error) {
      onWarning(errorText(error, t));
      return null;
    } finally {
      externalSelectionLock.current = false;
      setExternalSelectionBusy(false);
    }
  }, [onWarning, t]);

  return {
    projects,
    filteredProjects,
    search,
    setSearch,
    refresh,
    openFolder,
    selectExternalProject,
    externalSelectionBusy,
  };
}
