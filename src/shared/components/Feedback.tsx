import { useEffect, useId, useRef } from "react";
import {
  Anchor,
  AlertTriangle,
  CheckCircle2,
  Info,
  LoaderCircle,
  RefreshCw,
  Trash2,
  X,
  XCircle,
} from "lucide-react";
import type { ConfirmationState, ToastMessage } from "../../domain/types";
import type { Translate } from "../../i18n";
import { HarborMark } from "./LayoutPrimitives";

export function LoadingScreen() {
  return (
    <div className="loading-screen">
      <HarborMark large />
      <div>
        <strong>mendimaru</strong>
        <span>Studio Pro port</span>
      </div>
      <LoaderCircle size={20} className="spin" />
    </div>
  );
}

export function ConfigurationRecovery({
  t,
  detail,
  busy,
  onRecover,
}: {
  t: Translate;
  detail: string | null;
  busy: boolean;
  onRecover: () => void;
}) {
  return (
    <main className="recovery-screen">
      <HarborMark large />
      <span className="recovery-symbol" aria-hidden="true">
        <AlertTriangle size={22} />
      </span>
      <h1>{t("config-recovery-title")}</h1>
      <p>{t("config-recovery-detail")}</p>
      {detail && <code>{detail}</code>}
      <button
        type="button"
        className="button primary"
        onClick={onRecover}
        disabled={busy}
      >
        <RefreshCw size={16} className={busy ? "spin" : ""} />
        {t("action-recover-settings")}
      </button>
    </main>
  );
}

export function ToastStack({
  t,
  toasts,
  onDismiss,
}: {
  t: Translate;
  toasts: ToastMessage[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className="toast-stack" aria-live="polite">
      {toasts.map((toast) => (
        <div className={`toast ${toast.kind}`} key={toast.id}>
          {toast.kind === "success" ? (
            <CheckCircle2 size={18} />
          ) : toast.kind === "error" ? (
            <XCircle size={18} />
          ) : (
            <Info size={18} />
          )}
          <div>
            <strong>{toast.title}</strong>
            {toast.detail && <span>{toast.detail}</span>}
          </div>
          <button
            type="button"
            onClick={() => onDismiss(toast.id)}
            aria-label={t("dismiss-notification")}
          >
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}

export function ConfirmDialog({
  t,
  state,
  onCancel,
  onConfirm,
}: {
  t: Translate;
  state: ConfirmationState;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const previouslyFocused =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    cancelRef.current?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        dialogRef.current?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      );
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previouslyFocused?.focus();
    };
  }, [onCancel]);

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <div
        ref={dialogRef}
        className="confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <span className="dialog-symbol" aria-hidden="true">
          {state.danger ? <Trash2 size={21} /> : <Anchor size={21} />}
        </span>
        <h2 id={titleId}>{state.title}</h2>
        <p id={descriptionId}>{state.description}</p>
        <div>
          <button
            ref={cancelRef}
            type="button"
            className="button secondary"
            onClick={onCancel}
          >
            {t("action-cancel")}
          </button>
          <button
            type="button"
            className={`button ${state.danger ? "danger" : "primary"}`}
            onClick={onConfirm}
          >
            {state.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
