import { useEffect, useRef, useState } from "react";
import { tauriApi } from "../../api/tauri";
import type { FrontendHealth } from "../../domain/types";
import type { MessageKey, Translate } from "../../i18n";

export function FrontendHealthPanel({ t }: { t: Translate }) {
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<FrontendHealth | null>(null);
  const [failed, setFailed] = useState(false);
  const generation = useRef(0);
  const pending = useRef(false);
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  async function diagnose() {
    if (pending.current || !target.trim()) return;
    pending.current = true;
    const current = ++generation.current;
    setBusy(true);
    setReport(null);
    setFailed(false);
    try {
      const result = await tauriApi.diagnoseFrontendHealth(target.trim());
      if (current === generation.current) setReport(result);
    } catch {
      if (current === generation.current) setFailed(true);
    } finally {
      pending.current = false;
      if (current === generation.current) setBusy(false);
    }
  }
  const readiness = (value: boolean | null | undefined) =>
    t(
      value == null
        ? "frontend-not-checked"
        : value
          ? "frontend-ready"
          : "frontend-not-ready",
    );
  return (
    <section
      className="section-card frontend-health"
      aria-labelledby="frontend-health-heading"
    >
      <h2 id="frontend-health-heading">{t("frontend-title")}</h2>
      <p>{t("frontend-description")}</p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void diagnose();
        }}
      >
        <label className="simple-field">
          <span>{t("frontend-target")}</span>
          <input
            value={target}
            disabled={busy}
            onChange={(event) => {
              setTarget(event.target.value);
              setReport(null);
              setFailed(false);
            }}
            placeholder="http://localhost:8080/"
            maxLength={4096}
            required
          />
        </label>
        <button
          className="button secondary"
          disabled={busy || !target.trim()}
          type="submit"
        >
          {t(busy ? "frontend-checking" : "frontend-run")}
        </button>
      </form>
      <div role="status" aria-live="polite">
        {failed && <p>{t("frontend-failed")}</p>}
        <dl>
          <dt>{t("frontend-studio")}</dt>
          <dd>
            {report?.studioState
              ? t(`frontend-studio-${report.studioState}` as MessageKey)
              : t("frontend-not-checked")}
          </dd>
          <dt>{t("frontend-http")}</dt>
          <dd>{readiness(report?.httpReady)}</dd>
          <dt>{t("frontend-browser")}</dt>
          <dd>
            {report
              ? t(`frontend-${report.frontendState}` as MessageKey)
              : t("frontend-not-checked")}
          </dd>
        </dl>
        {report && (
          <>
            <p>
              {t("frontend-observation", {
                milliseconds: report.observationMilliseconds,
              })}
            </p>
            <ul>
              {report.diagnostics.map((diagnostic, index) => (
                <li key={index}>
                  <strong>
                    {t(
                      `frontend-cause-${diagnostic.code.replace(/_/g, "-")}` as MessageKey,
                    )}
                  </strong>
                  {diagnostic.endpoint && (
                    <code>
                      {diagnostic.endpoint.scheme} ·{" "}
                      {diagnostic.endpoint.hostKind} ·{" "}
                      {diagnostic.endpoint.port} ·{" "}
                      {diagnostic.endpoint.pathKind}
                      {diagnostic.status ? ` · HTTP ${diagnostic.status}` : ""}
                      {diagnostic.failure ? ` · ${diagnostic.failure}` : ""}
                    </code>
                  )}
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </section>
  );
}
