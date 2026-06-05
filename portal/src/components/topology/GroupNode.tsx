import { type NodeProps } from "@xyflow/react";
import { severityVar } from "../../lib/format";

// Pick a glyph for a group based on what it represents, so a k3d cluster, a
// Docker host, and a NixOS box are visually distinct at a glance.
function groupIcon(label: string): string | null {
  const l = label.toLowerCase();
  if (/k3d|k8s|kube|cluster|controller|daemonset|namespace/.test(l)) return "☸"; // ☸ k8s
  if (/docker|container|compose/.test(l)) return "🐳"; // 🐳 docker
  if (/nixos|nix/.test(l)) return "❄"; // ❄ nix
  if (/host|vm|linux|node|machine/.test(l)) return "🖥"; // 🖥 host
  return null;
}

export function GroupNode({ data }: NodeProps) {
  const d = data as { label: string; count: number; severity: string | null; dimmed?: boolean };
  const accent = d.severity ? severityVar(d.severity) : "rgb(var(--line))";
  const icon = groupIcon(d.label);
  return (
    <div
      className="h-full w-full rounded-xl border border-line bg-surface-2/30 transition-opacity"
      style={{ borderTop: `2px solid ${accent}`, opacity: d.dimmed ? 0.3 : 1 }}
    >
      <div className="flex items-center gap-2 border-b border-line px-3 py-1.5">
        <span className="h-2 w-2 rounded-full" style={{ background: accent }} />
        {icon && (
          <span className="text-[12px] leading-none" title={d.label} aria-hidden>
            {icon}
          </span>
        )}
        <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-fg-mute">{d.label}</span>
        <span className="rounded-full bg-surface-2 px-1.5 text-[10px] text-fg-mute">{d.count}</span>
      </div>
    </div>
  );
}
