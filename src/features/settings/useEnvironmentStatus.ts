import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { EnvironmentStatus } from "../../domain/types";
import type { EnvironmentStatusDependencies } from "./dependencies";

export function useEnvironmentStatus({
  t,
  onWarning,
}: EnvironmentStatusDependencies) {
  const [status, setStatus] = useState<EnvironmentStatus | null>(null);
  const latestRequest = useRef(0);

  const refreshStatus = useCallback(async () => {
    const requestId = ++latestRequest.current;
    try {
      const nextStatus = await tauriApi.getEnvironmentStatus();
      if (requestId === latestRequest.current) setStatus(nextStatus);
    } catch (error) {
      if (requestId === latestRequest.current) {
        onWarning(errorText(error, t));
      }
    }
  }, [onWarning, t]);

  useEffect(() => {
    const initialRefresh = window.setTimeout(() => void refreshStatus(), 0);
    const interval = window.setInterval(() => void refreshStatus(), 15_000);
    return () => {
      window.clearTimeout(initialRefresh);
      window.clearInterval(interval);
    };
  }, [refreshStatus]);

  return { status, refreshStatus };
}
