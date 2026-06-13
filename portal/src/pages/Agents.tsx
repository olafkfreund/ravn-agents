import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { listAgents, listEvents, type Agent } from "../lib/api";
import { relativeTime, statusMeta } from "../lib/format";
import { AgentDrawer } from "../components/AgentDrawer";

type StatusFilter = "online" | "stale" | "offline";

export function Agents() {
  const [search, setSearch] = useState("");
  const [active, setActive] = useState<Set<StatusFilter>>(new Set());
  const [selected, setSelected] = useState<Agent | null>(null);

  const agentsQ = useQuery({ queryKey: ["agents"], queryFn: listAgents, refetchInterval: 10_000 });
  const eventsQ = useQuery({ queryKey: ["events"], queryFn: () => listEvents(200), refetchInterval: 10_000 });

  const agents = agentsQ.data ?? [];
  const events = eventsQ.data ?? [];

  const counts = useMemo(() => {
    const c = { online: 0, stale: 0, offline: 0 } as Record<StatusFilter, number>;
    for (const a of agents) c[(a.status as StatusFilter) in c ? (a.status as StatusFilter) : "offline"]++;
    return c;
  }, [agents]);

  const eventsByAgent = useMemo(() => {
    const m = new Map<string, number>();
    for (const e of events) m.set(e.agent_id, (m.get(e.agent_id) ?? 0) + 1);
    return m;
  }, [events]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return agents.filter((a) => {
      if (active.size > 0 && !active.has(a.status as StatusFilter)) return false;
      if (!q) return true;
      const hay = `${a.host} ${Object.entries(a.labels ?? {}).map(([k, v]) => `${k}:${v}`).join(" ")}`.toLowerCase();
      return hay.includes(q);
    });
  }, [agents, active, search]);

  const toggle = (k: StatusFilter) =>
    setActive((prev) => {
      const next = new Set(prev);
      next.has(k) ? next.delete(k) : next.add(k);
      return next;
    });

  return (
    <div className="space-y-5">
      <div>
        <h2 className="font-display text-2xl font-bold tracking-tight">Agents</h2>
        <p className="text-sm text-fg-mute">Your fleet — status, labels, and recent activity.</p>
      </div>

      {/* Status summary */}
      <div className="grid grid-cols-3 gap-3">
        {(["online", "stale", "offline"] as StatusFilter[]).map((k) => {
          const m = statusMeta(k);
          return (
            <button
              key={k}
              onClick={() => toggle(k)}
              className={`flex items-center gap-3 rounded-xl border bg-surface p-4 text-left shadow-card transition-colors ${
                active.has(k) ? "border-accent" : "border-line hover:border-fg-mute"
              }`}
            >
              <span className="relative flex h-2.5 w-2.5 shrink-0">
                {k === "online" && <span className="animate-radar" />}
                <span className="relative inline-flex rounded-full h-2.5 w-2.5" style={{ background: m.dot }} />
              </span>
              <div>
                <p className={`font-display text-2xl font-bold tabular-nums ${m.text}`}>{counts[k]}</p>
                <p className="font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">{m.label}</p>
              </div>
            </button>
          );
        })}
      </div>

      <input
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        placeholder="Search host or label (e.g. env:prod)…"
        className="w-full rounded-lg border border-line bg-surface px-3 py-2 text-sm text-fg placeholder:text-fg-mute focus:border-accent focus-ring"
      />

      {agentsQ.isLoading && <p className="text-muted">Loading…</p>}
      {agentsQ.isError && (
        <div className="rounded-xl border border-sev-error/40 bg-sev-error/8 p-6 text-center text-sev-error">
          Couldn’t reach the control plane.
        </div>
      )}

      {agentsQ.data && filtered.length === 0 && (
        <div className="rounded-xl border border-dashed border-line bg-surface p-12 text-center">
          <p className="text-3xl">❖</p>
          <p className="mt-2 font-display text-lg font-bold">
            {agents.length === 0 ? "No agents yet" : "No matching agents"}
          </p>
          <p className="mt-1 text-sm text-fg-mute">
            {agents.length === 0 ? "Agents register automatically once they emit events." : "Try clearing the filter."}
          </p>
        </div>
      )}

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {filtered.map((a) => {
          const m = statusMeta(a.status);
          const labels = Object.entries(a.labels ?? {});
          return (
            <button
              key={a.agent_id}
              onClick={() => setSelected(a)}
              className="flex flex-col gap-2 rounded-xl border border-line bg-surface p-4 text-left shadow-card transition-colors hover:border-accent"
            >
              <div className="flex items-center justify-between gap-2 w-full">
                <span className="flex items-center gap-2 font-semibold min-w-0">
                  <span className="relative flex h-2 w-2 shrink-0">
                    {a.status === "online" && <span className="animate-radar" />}
                    <span className="relative inline-flex rounded-full h-2 w-2" style={{ background: m.dot }} />
                  </span>
                  <span className="truncate">{a.host}</span>
                </span>
                {a.status === "online" ? (
                  <div className="flex items-center gap-1.5 shrink-0">
                    <svg
                      className="h-4 w-12 text-sev-notice"
                      viewBox="0 0 50 20"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.75"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    >
                      <path className="animate-ekg-bg" opacity="0.15" d="M 0 10 H 12 L 15 3 L 18 17 L 21 0 L 24 20 L 27 10 H 30 L 33 6 L 36 10 H 50" />
                      <path className="animate-ekg-pulse" d="M 0 10 H 12 L 15 3 L 18 17 L 21 0 L 24 20 L 27 10 H 30 L 33 6 L 36 10 H 50" />
                    </svg>
                    <span className={`font-mono text-[10px] uppercase tracking-wider ${m.text}`}>{m.label}</span>
                  </div>
                ) : (
                  <span className={`font-mono text-[10px] uppercase tracking-wider ${m.text}`}>{m.label}</span>
                )}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {labels.length > 0 ? (
                  labels.map(([k, v]) => (
                    <span key={k} className="rounded-full border border-line bg-surface-2 px-1.5 py-0.5 text-[11px]">
                      <span className="text-fg-mute">{k}:</span> <span className="text-accent-2">{v}</span>
                    </span>
                  ))
                ) : (
                  <span className="text-[11px] text-fg-mute">no labels</span>
                )}
              </div>
              <div className="flex items-center justify-between font-mono text-[11px] text-fg-mute">
                <span>{eventsByAgent.get(a.agent_id) ?? 0} events</span>
                <span>seen {relativeTime(a.last_seen)}</span>
              </div>
            </button>
          );
        })}
      </div>

      <AgentDrawer agent={selected} events={events} onClose={() => setSelected(null)} />
    </div>
  );
}
