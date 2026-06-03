import { Handle, Position, type NodeProps } from "@xyflow/react";
import { severityVar, statusMeta } from "../../lib/format";

export function AgentNode({ data }: NodeProps) {
  const d = data as { host: string; status: string; severity: string | null };
  const sm = statusMeta(d.status);
  const border = d.severity ? severityVar(d.severity) : "rgb(var(--line))";
  return (
    <div
      className="rounded-lg border border-line bg-surface px-3 py-2 shadow-card"
      style={{ borderLeft: `3px solid ${border}`, width: 150 }}
    >
      <div className="flex items-center gap-1.5">
        <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: sm.dot }} />
        <span className="truncate text-sm font-medium text-fg">{d.host}</span>
      </div>
      <div className="mt-0.5 font-mono text-[10px] text-fg-mute">{d.severity ?? "quiet"}</div>
      <Handle type="target" position={Position.Top} style={{ opacity: 0 }} />
      <Handle type="source" position={Position.Bottom} style={{ opacity: 0 }} />
    </div>
  );
}
