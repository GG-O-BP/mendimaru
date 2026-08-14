import { useCallback, useRef, useState } from "react";

type ErrorHandler = (error: unknown) => void;

export function useActionRegistry(onError: ErrorHandler) {
  const locks = useRef<Set<string>>(new Set());
  const [busyActions, setBusyActions] = useState<Set<string>>(new Set());

  const runAction = useCallback(
    async (key: string, action: () => Promise<void>) => {
      if (locks.current.has(key)) return;
      locks.current.add(key);
      setBusyActions((current) => new Set(current).add(key));
      try {
        await action();
      } catch (error) {
        onError(error);
      } finally {
        locks.current.delete(key);
        setBusyActions((current) => {
          const next = new Set(current);
          next.delete(key);
          return next;
        });
      }
    },
    [onError],
  );

  const isBusy = useCallback(
    (key: string) => busyActions.has(key),
    [busyActions],
  );
  const hasBusyPrefix = useCallback(
    (prefix: string) =>
      Array.from(busyActions).some((key) => key.startsWith(prefix)),
    [busyActions],
  );

  return { runAction, isBusy, hasBusyPrefix };
}
