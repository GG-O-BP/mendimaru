import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { LocalizationBundle } from "../../domain/types";
import {
  applyDocumentLocale,
  createTranslate,
  loadLocalization,
  selectLanguage,
} from "../../i18n";

export function useLocalization(onError: (error: unknown) => void) {
  const [localization, setLocalization] = useState<LocalizationBundle | null>(
    null,
  );
  const [changing, setChanging] = useState(false);
  const changeLock = useRef(false);

  const applyLocalization = useCallback((bundle: LocalizationBundle) => {
    setLocalization(bundle);
    applyDocumentLocale(bundle);
    return bundle;
  }, []);

  useEffect(() => {
    let active = true;
    void loadLocalization()
      .then((bundle) => {
        if (active) applyLocalization(bundle);
      })
      .catch((error: unknown) => {
        if (active) onError(error);
      });
    return () => {
      active = false;
    };
  }, [applyLocalization, onError]);

  const changeLanguage = useCallback(
    async (language: string) => {
      if (changeLock.current) return undefined;
      changeLock.current = true;
      setChanging(true);
      try {
        return applyLocalization(await selectLanguage(language));
      } catch (error) {
        onError(error);
        return undefined;
      } finally {
        changeLock.current = false;
        setChanging(false);
      }
    },
    [applyLocalization, onError],
  );

  const t = useMemo(() => createTranslate(localization), [localization]);

  return {
    localization,
    t,
    changing,
    changeLanguage,
  };
}
