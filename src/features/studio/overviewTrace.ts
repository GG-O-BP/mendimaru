const traceStartedAt = performance.now();

export type StudioOverviewStage =
  | "workspace-ready"
  | "cache-request-start"
  | "cache-request-end"
  | "cache-render"
  | "installed-request-start"
  | "installed-request-end"
  | "installed-request-error"
  | "session-request-start"
  | "session-request-end"
  | "session-request-error"
  | "overview-ready";

export function traceStudioOverview(
  stage: StudioOverviewStage,
  detail: Record<string, number | boolean> = {},
) {
  if (!import.meta.env.DEV) return;
  const elapsedMs = Math.round((performance.now() - traceStartedAt) * 10) / 10;
  performance.mark(`mendimaru:studio-overview:${stage}`);
  if (import.meta.env.MODE !== "test") {
    console.debug("[studio-overview]", { stage, elapsedMs, ...detail });
  }
}
