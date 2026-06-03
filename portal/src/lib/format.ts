// Presentation helpers: severity/source metadata and time formatting.

export type SeverityKey = "info" | "notice" | "warning" | "error" | "critical";

export interface SeverityMeta {
  key: SeverityKey;
  label: string;
  /** Tailwind text color class bound to the severity ramp. */
  text: string;
  /** Background tint class. */
  tint: string;
  /** Border color class. */
  border: string;
  /** Rank for sorting/escalation (higher = more urgent). */
  rank: number;
}

const SEVERITY: Record<SeverityKey, SeverityMeta> = {
  info: { key: "info", label: "Info", text: "text-sev-info", tint: "bg-sev-info/10", border: "border-sev-info/30", rank: 1 },
  notice: { key: "notice", label: "Notice", text: "text-sev-notice", tint: "bg-sev-notice/10", border: "border-sev-notice/30", rank: 2 },
  warning: { key: "warning", label: "Warning", text: "text-sev-warning", tint: "bg-sev-warning/10", border: "border-sev-warning/30", rank: 3 },
  error: { key: "error", label: "Error", text: "text-sev-error", tint: "bg-sev-error/10", border: "border-sev-error/30", rank: 4 },
  critical: { key: "critical", label: "Critical", text: "text-sev-critical", tint: "bg-sev-critical/10", border: "border-sev-critical/40", rank: 5 },
};

export const SEVERITY_ORDER: SeverityKey[] = ["critical", "error", "warning", "notice", "info"];

export function severityMeta(severity: string): SeverityMeta {
  return SEVERITY[(severity as SeverityKey)] ?? SEVERITY.info;
}

/** Resolved color for a severity's rail/dot (channels wrapped in rgb()). */
export function severityVar(severity: string): string {
  const key = severity in SEVERITY ? severity : "info";
  return `rgb(var(--sev-${key}))`;
}

const SOURCE_LABELS: Record<string, string> = {
  journald: "journald",
  failed_unit: "failed unit",
  config_drift: "config drift",
  auth: "auth",
  update: "update",
};

export function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? source.replace(/_/g, " ");
}

/** Compact relative time, e.g. "3m ago", "2h ago", "just now". */
export function relativeTime(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return iso;
  const secs = Math.round((Date.now() - then) / 1000);
  if (secs < 0) return "in the future";
  if (secs < 45) return "just now";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.round(hrs / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(then).toLocaleDateString();
}

/** Absolute timestamp for tooltips/detail. */
export function absoluteTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}
