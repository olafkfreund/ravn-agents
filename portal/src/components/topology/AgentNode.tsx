import { Handle, Position, type NodeProps } from "@xyflow/react";
import { severityVar, statusMeta } from "../../lib/format";

export interface AgentNodeData {
  host: string;
  status: string;
  severity: string | null;
  dimmed?: boolean;
  labels?: Record<string, string>;
}

function getProviderBadge(labels?: Record<string, string>): { text: string; bg: string; textClass: string } | null {
  if (!labels) return null;
  const kind = (labels.kind || "").toLowerCase();
  
  if (kind.includes("gke")) {
    return { text: "GKE", bg: "bg-blue-500/10 border-blue-500/20", textClass: "text-blue-400" };
  }
  if (kind.includes("eks")) {
    return { text: "EKS", bg: "bg-amber-500/10 border-amber-500/20", textClass: "text-amber-400" };
  }
  if (kind.includes("aks")) {
    return { text: "AKS", bg: "bg-sky-500/10 border-sky-500/20", textClass: "text-sky-400" };
  }
  if (kind.includes("k3d")) {
    return { text: "k3d", bg: "bg-purple-500/10 border-purple-500/20", textClass: "text-purple-400" };
  }
  if (kind.includes("nixos")) {
    return { text: "NixOS", bg: "bg-cyan-500/10 border-cyan-500/20", textClass: "text-cyan-400" };
  }
  if (kind.endsWith("-cluster")) {
    const name = labels.kind.replace(/-cluster$/i, "").toUpperCase();
    return { text: name, bg: "bg-teal-500/10 border-teal-500/20", textClass: "text-teal-400" };
  }
  return null;
}

export function AgentNode({ data }: NodeProps) {
  const d = data as unknown as AgentNodeData;
  const sm = statusMeta(d.status);
  const border = d.severity ? severityVar(d.severity) : "rgb(var(--line))";
  const badge = getProviderBadge(d.labels);

  return (
    <div
      className="group relative rounded-lg border border-line bg-surface px-3 py-2 shadow-card transition-all duration-200 hover:border-accent hover:shadow-md cursor-pointer select-none"
      style={{
        borderLeft: `3px solid ${border}`,
        width: 150,
        opacity: d.dimmed ? 0.25 : 1,
      }}
    >
      <div className="flex items-center justify-between gap-1">
        <div className="flex items-center gap-1.5 min-w-0">
          <span className="relative flex h-2 w-2 shrink-0">
            {d.status === "online" && <span className="animate-radar" />}
            <span
              className="relative inline-flex rounded-full h-2 w-2 shrink-0"
              style={{
                background: sm.dot,
                boxShadow: d.status === "online" ? `0 0 6px ${sm.dot}` : "none"
              }}
            />
          </span>
          <span className="truncate text-sm font-medium text-fg group-hover:text-accent transition-colors">{d.host}</span>
        </div>
      </div>
      
      <div className="mt-1 flex items-center justify-between">
        <span className="font-mono text-[9px] text-fg-mute font-medium uppercase tracking-wider">{d.severity ?? "quiet"}</span>
        {badge && (
          <span className={`rounded px-1 py-[1px] font-mono text-[8px] font-bold border ${badge.bg} ${badge.textClass}`}>
            {badge.text}
          </span>
        )}
      </div>

      <Handle type="target" position={Position.Top} style={{ opacity: 0 }} />
      <Handle type="source" position={Position.Bottom} style={{ opacity: 0 }} />
    </div>
  );
}
