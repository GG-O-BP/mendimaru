import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { EnvironmentStatus } from "../../domain/types";
import type { EnvironmentStatusDependencies } from "./dependencies";

const REFRESH_INTERVAL_MILLISECONDS = 15_000;

export interface EnvironmentRefreshOptions {
  sourceChanged?: boolean;
}

type RefreshOutcome =
  { ok: true; status: EnvironmentStatus } | { ok: false; error: unknown };

interface PendingRefresh {
  generation: number;
  promise: Promise<RefreshOutcome>;
}

export function useEnvironmentStatus({
  t,
  onWarning,
}: EnvironmentStatusDependencies) {
  const [status, setStatus] = useState<EnvironmentStatus | null>(null);
  const mounted = useRef(false);
  const sourceGeneration = useRef(0);
  const pendingRefresh = useRef<PendingRefresh | null>(null);
  const lastWarning = useRef<string | null>(null);

  const requestForGeneration = useCallback(async (generation: number) => {
    for (;;) {
      let pending = pendingRefresh.current;
      if (!pending) {
        const promise = tauriApi
          .getEnvironmentStatus()
          .then<RefreshOutcome, RefreshOutcome>(
            (nextStatus) => ({ ok: true, status: nextStatus }),
            (error: unknown) => ({ ok: false, error }),
          );
        pending = { generation, promise };
        pendingRefresh.current = pending;
        void promise.then(() => {
          if (pendingRefresh.current === pending) pendingRefresh.current = null;
        });
      }

      const outcome = await pending.promise;
      if (pendingRefresh.current === pending) pendingRefresh.current = null;
      if (pending.generation === generation) return outcome;
      if (sourceGeneration.current !== generation) return null;
    }
  }, []);

  const refreshStatus = useCallback(
    async (options: EnvironmentRefreshOptions = {}) => {
      if (options.sourceChanged) sourceGeneration.current += 1;
      const generation = sourceGeneration.current;
      const outcome = await requestForGeneration(generation);
      if (
        !outcome ||
        !mounted.current ||
        generation !== sourceGeneration.current
      ) {
        return;
      }

      if (outcome.ok) {
        setStatus(outcome.status);
        if (lastWarning.current !== null) {
          lastWarning.current = null;
          onWarning(null);
        }
        return;
      }

      const warning = errorText(outcome.error, t);
      if (warning !== lastWarning.current) {
        lastWarning.current = warning;
        onWarning(warning);
      }
    },
    [onWarning, requestForGeneration, t],
  );

  useEffect(() => {
    mounted.current = true;
    let active = true;
    let timeout: number | undefined;

    const poll = async () => {
      await refreshStatus();
      if (active) {
        timeout = window.setTimeout(poll, REFRESH_INTERVAL_MILLISECONDS);
      }
    };
    timeout = window.setTimeout(poll, 0);

    return () => {
      active = false;
      mounted.current = false;
      if (timeout !== undefined) window.clearTimeout(timeout);
    };
  }, [refreshStatus]);

  return { status, refreshStatus };
}
