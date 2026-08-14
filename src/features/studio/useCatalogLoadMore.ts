import { useEffect, useRef } from "react";

export function useCatalogLoadMore({
  hasMore,
  loading,
  error,
  onLoadMore,
}: {
  hasMore: boolean;
  loading: boolean;
  error: string | null;
  onLoadMore: () => void;
}) {
  const sentinelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore || loading || error) return undefined;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return;
        observer.disconnect();
        onLoadMore();
      },
      {
        root: sentinel.closest(".page"),
        rootMargin: "0px 0px 240px 0px",
      },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [error, hasMore, loading, onLoadMore]);

  return sentinelRef;
}
