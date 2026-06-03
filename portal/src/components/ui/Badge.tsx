import type { ReactNode } from "react";

type Tone = "info" | "notice" | "warning" | "error" | "critical" | "neutral";

const TONES: Record<Tone, string> = {
  info: "text-blue border-blue/40",
  notice: "text-accent-2 border-accent-2/40",
  warning: "text-yellow border-yellow/40",
  error: "text-red border-red/40",
  critical: "text-red border-red/60 font-semibold",
  neutral: "text-muted border-border",
};

/** Map an event severity string to a badge tone. */
export function severityTone(severity: string): Tone {
  switch (severity) {
    case "info":
    case "notice":
    case "warning":
    case "error":
    case "critical":
      return severity;
    default:
      return "neutral";
  }
}

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return (
    <span
      className={`inline-flex items-center rounded-full border bg-bg-soft px-2 py-0.5 text-xs ${TONES[tone]}`}
    >
      {children}
    </span>
  );
}
