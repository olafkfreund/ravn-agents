import { type NodeProps } from "@xyflow/react";
import { severityVar } from "../../lib/format";

export function GroupNode({ data }: NodeProps) {
  const d = data as { label: string; count: number; severity: string | null; dimmed?: boolean };
  const accent = d.severity ? severityVar(d.severity) : "rgb(var(--line))";
  return (
    <div
      className="h-full w-full rounded-xl border border-line bg-surface-2/30 transition-opacity"
      style={{ borderTop: `2px solid ${accent}`, opacity: d.dimmed ? 0.3 : 1 }}
    >
      <div className="flex items-center gap-2 border-b border-line px-3 py-1.5">
        <span className="h-2 w-2 rounded-full" style={{ background: accent }} />
        <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-fg-mute">{d.label}</span>
        <span className="rounded-full bg-surface-2 px-1.5 text-[10px] text-fg-mute">{d.count}</span>
      </div>
    </div>
  );
}
