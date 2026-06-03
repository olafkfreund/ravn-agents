import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ReactFlow, Background, Controls, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { getTopology, type Topology as TopologyData } from "../lib/api";
import { SEVERITY_ORDER, severityMeta, severityVar, type SeverityKey } from "../lib/format";
import { AgentNode } from "../components/topology/AgentNode";
import { GroupNode } from "../components/topology/GroupNode";

const nodeTypes = { agent: AgentNode, group: GroupNode };

const COLS = 2;
const NW = 160;
const NH = 70;
const PAD = 16;
const HEADER = 34;
const GAP = 44;
const MAX_ROW = 1180;

function worstSeverity(sevs: (string | null | undefined)[]): string | null {
  let best: string | null = null;
  let bestRank = 0;
  for (const s of sevs) {
    if (!s) continue;
    const r = severityMeta(s).rank;
    if (r > bestRank) {
      bestRank = r;
      best = s;
    }
  }
  return best;
}

interface Filter {
  q: string;
  sev: Set<SeverityKey>;
}

/** Build group + agent nodes with a wrapped-grid layout and filter dimming. */
function buildNodes(t: TopologyData | undefined, filter: Filter): Node[] {
  if (!t) return [];
  const active = filter.q.trim() !== "" || filter.sev.size > 0;
  const q = filter.q.trim().toLowerCase();
  const matches = (host: string, severity: string | null | undefined) => {
    if (q && !host.toLowerCase().includes(q)) return false;
    if (filter.sev.size > 0 && !(severity && filter.sev.has(severity as SeverityKey))) return false;
    return true;
  };

  const nodes: Node[] = [];
  let x = 0;
  let y = 0;
  let rowHeight = 0;

  for (const g of t.groups) {
    const n = Math.max(g.nodes.length, 1);
    const rows = Math.ceil(n / COLS);
    const cols = Math.min(COLS, n);
    const gw = cols * NW + (cols + 1) * PAD;
    const gh = HEADER + rows * NH + (rows + 1) * PAD;

    if (x > 0 && x + gw > MAX_ROW) {
      x = 0;
      y += rowHeight + GAP;
      rowHeight = 0;
    }

    const gid = `g:${g.key}`;
    const groupSeverity = worstSeverity(g.nodes.map((node) => node.severity));
    const groupMatch = active ? g.nodes.some((node) => matches(node.host, node.severity)) : true;

    nodes.push({
      id: gid,
      type: "group",
      position: { x, y },
      data: { label: g.key, count: g.nodes.length, severity: groupSeverity, dimmed: active && !groupMatch },
      style: { width: gw, height: gh },
      draggable: false,
      selectable: false,
    });

    g.nodes.forEach((node, i) => {
      const col = i % COLS;
      const row = Math.floor(i / COLS);
      nodes.push({
        id: node.agent_id,
        type: "agent",
        parentId: gid,
        extent: "parent",
        position: { x: PAD + col * (NW + PAD), y: HEADER + PAD + row * (NH + PAD) },
        data: {
          host: node.host,
          status: node.status,
          severity: node.severity,
          dimmed: active && !matches(node.host, node.severity),
        },
        draggable: false,
      });
    });

    x += gw + GAP;
    rowHeight = Math.max(rowHeight, gh);
  }
  return nodes;
}

export function Topology() {
  const [groupBy, setGroupBy] = useState("");
  const [q, setQ] = useState("");
  const [sev, setSev] = useState<Set<SeverityKey>>(new Set());

  const { data, isLoading, isError } = useQuery({
    queryKey: ["topology", groupBy],
    queryFn: () => getTopology(groupBy || undefined),
    refetchInterval: 10_000,
  });

  const nodes = useMemo(() => buildNodes(data, { q, sev }), [data, q, sev]);
  const dimensions = data?.dimensions ?? [];
  const toggleSev = (k: SeverityKey) =>
    setSev((prev) => {
      const next = new Set(prev);
      next.has(k) ? next.delete(k) : next.add(k);
      return next;
    });

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="font-display text-2xl font-bold tracking-tight">Topology</h2>
          <p className="text-sm text-fg-mute">Your fleet, grouped by a category of your choosing.</p>
        </div>
        <label className="flex items-center gap-2 text-sm">
          <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-fg-mute">Group by</span>
          <select
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value)}
            className="rounded-lg border border-line bg-surface px-2.5 py-1.5 text-sm text-fg focus:border-accent focus-ring"
          >
            <option value="">— none —</option>
            {dimensions.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
      </div>

      {/* Filters */}
      <div className="flex flex-col gap-3 lg:flex-row lg:items-center">
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="Search host…"
          className="flex-1 rounded-lg border border-line bg-surface px-3 py-2 text-sm text-fg placeholder:text-fg-mute focus:border-accent focus-ring"
        />
        <div className="flex flex-wrap items-center gap-1.5">
          {SEVERITY_ORDER.map((k) => {
            const m = severityMeta(k);
            return (
              <button
                key={k}
                onClick={() => toggleSev(k)}
                className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-all ${
                  sev.has(k) ? `${m.tint} ${m.border} ${m.text}` : "border-line text-fg-dim hover:border-fg-mute"
                }`}
              >
                <span className="h-2 w-2 rounded-full" style={{ background: severityVar(k) }} />
                {m.label}
              </button>
            );
          })}
        </div>
      </div>

      <div className="h-[64vh] overflow-hidden rounded-xl border border-line bg-surface">
        {isError ? (
          <div className="grid h-full place-items-center text-sev-error">Couldn’t reach the control plane.</div>
        ) : isLoading ? (
          <div className="grid h-full place-items-center text-fg-mute">Loading…</div>
        ) : nodes.length === 0 ? (
          <div className="grid h-full place-items-center text-fg-mute">No agents yet.</div>
        ) : (
          <ReactFlow
            key={groupBy}
            nodes={nodes}
            edges={[]}
            nodeTypes={nodeTypes}
            colorMode="system"
            fitView
            nodesDraggable={false}
            nodesConnectable={false}
            proOptions={{ hideAttribution: true }}
          >
            <Background gap={20} />
            <Controls showInteractive={false} />
          </ReactFlow>
        )}
      </div>
    </div>
  );
}
