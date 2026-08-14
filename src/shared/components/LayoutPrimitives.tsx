import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

export function HarborMark({ large = false }: { large?: boolean }) {
  return (
    <span className={`harbor-mark ${large ? "large" : ""}`} aria-hidden="true">
      <img src="/mendimaru.png" alt="" draggable={false} />
    </span>
  );
}

export function PageTitle({
  eyebrow,
  title,
  description,
}: {
  eyebrow: string;
  title: string;
  description: string;
}) {
  return (
    <header className="page-title">
      <span>{eyebrow}</span>
      <h1>{title}</h1>
      <p>{description}</p>
    </header>
  );
}

export function SectionHeader({
  id,
  title,
  count,
  meta,
  action,
}: {
  id?: string;
  title: string;
  count?: ReactNode;
  meta?: string;
  action?: ReactNode;
}) {
  return (
    <div className="section-header">
      <div>
        <h2 id={id}>{title}</h2>
        {count != null && <b>{count}</b>}
        {meta && <span>{meta}</span>}
      </div>
      {action}
    </div>
  );
}

export function EmptyState({
  icon: Icon,
  title,
  detail,
}: {
  icon: LucideIcon;
  title: string;
  detail: string;
}) {
  return (
    <div className="empty-state">
      <Icon size={23} />
      <div>
        <strong>{title}</strong>
        <span>{detail}</span>
      </div>
    </div>
  );
}
