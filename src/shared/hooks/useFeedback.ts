import { useCallback, useEffect, useRef, useState } from "react";
import type {
  ConfirmationState,
  ToastKind,
  ToastMessage,
} from "../../domain/types";

export function useFeedback() {
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  const [confirmation, setConfirmation] = useState<ConfirmationState | null>(
    null,
  );
  const nextToastId = useRef(0);
  const timers = useRef<Map<number, number>>(new Map());

  useEffect(
    () => () => {
      for (const timer of timers.current.values()) window.clearTimeout(timer);
      timers.current.clear();
    },
    [],
  );

  const dismissToast = useCallback((id: number) => {
    const timer = timers.current.get(id);
    if (timer != null) window.clearTimeout(timer);
    timers.current.delete(id);
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const notify = useCallback(
    (kind: ToastKind, title: string, detail?: string) => {
      const id = ++nextToastId.current;
      setToasts((current) => [...current, { id, kind, title, detail }]);
      timers.current.set(
        id,
        window.setTimeout(() => dismissToast(id), 5_000),
      );
    },
    [dismissToast],
  );

  const requestConfirmation = useCallback((state: ConfirmationState) => {
    setConfirmation(state);
  }, []);

  const cancelConfirmation = useCallback(() => setConfirmation(null), []);
  const acceptConfirmation = useCallback(() => {
    const action = confirmation?.action;
    setConfirmation(null);
    if (action) void action();
  }, [confirmation]);

  const clearFeedback = useCallback(() => {
    for (const timer of timers.current.values()) window.clearTimeout(timer);
    timers.current.clear();
    setToasts([]);
    setConfirmation(null);
  }, []);

  return {
    toasts,
    confirmation,
    notify,
    dismissToast,
    requestConfirmation,
    cancelConfirmation,
    acceptConfirmation,
    clearFeedback,
  };
}
