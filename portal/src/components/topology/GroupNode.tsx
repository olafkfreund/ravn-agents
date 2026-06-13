import { type NodeProps } from "@xyflow/react";
import { severityVar } from "../../lib/format";

export interface GroupNodeData {
  label: string;
  count: number;
  severity: string | null;
  dimmed?: boolean;
  nodes?: Array<{
    agent_id: string;
    host: string;
    status: string;
    severity: string | null;
    labels: Record<string, string>;
  }>;
}

interface ProviderMeta {
  name: string;
  icon: string;
  badgeClass: string;
  dotColor: string;
  bgClass: string;
}

function getProviderMeta(label: string, nodes?: GroupNodeData["nodes"]): ProviderMeta {
  const l = label.toLowerCase();
  
  // 1. Check if group label has provider cues
  let provider = "";
  let customClusterName = "";
  if (/gke/.test(l)) provider = "gke";
  else if (/eks/.test(l)) provider = "eks";
  else if (/aks/.test(l)) provider = "aks";
  else if (/k3d|ravn-dev/.test(l)) provider = "k3d";
  else if (l.endsWith("-cluster")) {
    customClusterName = label.replace(/-cluster$/i, "").toUpperCase();
  }
  
  // 2. Check individual node labels if we have them
  if (!provider && !customClusterName && nodes && nodes.length > 0) {
    for (const node of nodes) {
      const kind = (node.labels?.kind || "").toLowerCase();
      if (kind.includes("gke")) { provider = "gke"; break; }
      if (kind.includes("eks")) { provider = "eks"; break; }
      if (kind.includes("aks")) { provider = "aks"; break; }
      if (kind.includes("k3d")) { provider = "k3d"; break; }
      if (kind.endsWith("-cluster")) {
        customClusterName = node.labels.kind.replace(/-cluster$/i, "").toUpperCase();
        break;
      }
    }
  }

  if (customClusterName) {
    return {
      name: `${customClusterName} Cluster`,
      icon: "☸️",
      badgeClass: "bg-teal-500/10 text-teal-400 border-teal-500/20",
      dotColor: "#00F5D4",
      bgClass: "bg-teal-950/5 border-teal-500/10",
    };
  }

  // Fallback to label keyword matching for other kinds
  if (provider === "gke") {
    return {
      name: "Google GKE",
      icon: "☸️",
      badgeClass: "bg-blue-500/10 text-blue-400 border-blue-500/20",
      dotColor: "#4285F4",
      bgClass: "bg-blue-950/5 border-blue-500/10",
    };
  }
  if (provider === "eks") {
    return {
      name: "AWS EKS",
      icon: "☸️",
      badgeClass: "bg-amber-500/10 text-amber-400 border-amber-500/20",
      dotColor: "#FF9900",
      bgClass: "bg-amber-950/5 border-amber-500/10",
    };
  }
  if (provider === "aks") {
    return {
      name: "Azure AKS",
      icon: "☸️",
      badgeClass: "bg-sky-500/10 text-sky-400 border-sky-500/20",
      dotColor: "#0078D4",
      bgClass: "bg-sky-950/5 border-sky-500/10",
    };
  }
  if (provider === "k3d") {
    return {
      name: "k3d Cluster",
      icon: "☸️",
      badgeClass: "bg-purple-500/10 text-purple-400 border-purple-500/20",
      dotColor: "#9061F9",
      bgClass: "bg-purple-950/5 border-purple-500/10",
    };
  }

  // NixOS host check
  if (/nixos|nix/.test(l)) {
    return {
      name: "NixOS Host",
      icon: "❄️",
      badgeClass: "bg-cyan-500/10 text-cyan-400 border-cyan-500/20",
      dotColor: "#86c0d8",
      bgClass: "bg-cyan-950/5 border-cyan-500/10",
    };
  }

  // General host check
  if (/host|vm|linux|node|machine/.test(l)) {
    return {
      name: "Linux Host",
      icon: "🖥️",
      badgeClass: "bg-emerald-500/10 text-emerald-400 border-emerald-500/20",
      dotColor: "#10B981",
      bgClass: "bg-emerald-950/5 border-emerald-500/10",
    };
  }

  return {
    name: "General Group",
    icon: "📁",
    badgeClass: "bg-zinc-500/10 text-zinc-400 border-zinc-500/20",
    dotColor: "rgb(var(--line))",
    bgClass: "bg-surface-2/10 border-line/50",
  };
}

export function GroupNode({ data }: NodeProps) {
  const d = data as unknown as GroupNodeData;
  const severityColor = d.severity ? severityVar(d.severity) : null;
  const pm = getProviderMeta(d.label, d.nodes);
  
  // Decide which accent color to use for the top border and dot
  const accentColor = severityColor || pm.dotColor;

  return (
    <div
      className={`h-full w-full rounded-xl border ${pm.bgClass} shadow-card transition-all duration-300`}
      style={{
        borderTop: `3px solid ${accentColor}`,
        opacity: d.dimmed ? 0.3 : 1,
      }}
    >
      <div className="flex items-center justify-between border-b border-line/40 px-3.5 py-2">
        <div className="flex items-center gap-2">
          <span className="h-2 w-2 rounded-full animate-pulse-subtle" style={{ background: accentColor }} />
          <span className="font-mono text-xs font-bold uppercase tracking-wider text-fg">{d.label}</span>
          <span className="rounded-full bg-surface-2 border border-line/60 px-2 py-0.5 font-mono text-[10px] text-fg-mute font-medium">{d.count}</span>
        </div>
        <div className={`flex items-center gap-1 rounded-md border px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wider shadow-sm whitespace-nowrap shrink-0 ${pm.badgeClass}`}>
          <span>{pm.icon}</span>
          <span>{pm.name}</span>
        </div>
      </div>
    </div>
  );
}
