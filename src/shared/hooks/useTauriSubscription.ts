import { useEffect } from "react";

type Unsubscribe = () => void;
type Subscribe = () => Promise<Unsubscribe>;
type SubscriptionErrorHandler = (error: unknown) => void;

export function useTauriSubscription(
  subscribe: Subscribe,
  onError?: SubscriptionErrorHandler,
) {
  useEffect(() => {
    let disposed = false;
    let unsubscribe: Unsubscribe | undefined;

    void Promise.resolve()
      .then(subscribe)
      .then((nextUnsubscribe) => {
        if (disposed) {
          nextUnsubscribe();
        } else {
          unsubscribe = nextUnsubscribe;
        }
      })
      .catch((error: unknown) => {
        if (!disposed) onError?.(error);
      });

    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [onError, subscribe]);
}
