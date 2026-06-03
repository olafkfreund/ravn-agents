import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ReactFlow, Background, Controls, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { getTopology, type Topology as TopologyData } from "../lib/api";
import { AgentNode } from "../components/topology/AgentNode";
import { GroupNode } from "../components/topology/GroupNode";

const nodeTypes = { agent: AgentNode, group: GroupNode };

const COLS = 2;
const NW = 160;
const NH = 70;
const PAD = 16;
const HEADER = 34;
const GAP = 44;

function buildNodes(t?: TopologyData): Node[] {
  if (!t) return [];
  const nodes: Node[] = [];
  let x = 0;
  for (const g of t.groups) {
    const n = Math.max(g.nodes.length, 1);
    const rows = Math.ceil(n / COLS);
    const cols = Math.min(COLS, n);
    const gw = cols * NW + (cols + 1) * PAD;
    const gh = HEADER + rows * NH + (rows + 1) * PAD;
    const gid = `g:${g.key}`;
    nodes.push({
      id: gid,
      type: "group",
      position: { x, y: 0 },
      data: { label: g.key, count: g.nodes.length },
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
        data: { host: node.host, status: node.status, severity: node.severity },
        draggable: false,
      });
    });
    x += gw + GAP;
  }
  return nodes;
}

export function Topology() {
  const [groupBy, setGroupBy] = useState("");
  const { data, isLoading, isError } = useQuery({
    queryKey: ["topology", groupBy],
    queryFn: () => getTopology(groupBy || undefined),
    refetchInterval: 10_000,
  });

  const nodes = useMemo(() => buildNodes(data), [data]);
  const dimensions = data?.dimensions ?? [];

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

      <div className="h-[68vh] overflow-hidden rounded-xl border border-line bg-surface">
        {isError ? (
          <div className="grid h-full place-items-center text-sev-error">Couldn’t reach the control plane.</div>
        ) : isLoading ? (
          <div className="grid h-full place-items-center text-fg-mute">Loading…</div>
        ) : nodes.length === 0 ? (
          <div className="grid h-full place-items-center text-fg-mute">No agents yet.</div>
        ) : (
          <ReactFlow
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
