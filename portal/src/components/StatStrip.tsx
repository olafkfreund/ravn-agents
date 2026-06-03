import { useMemo } from "react";
import type { StoredEvent } from "../lib/api";
import { relativeTime, severityMeta } from "../lib/format";

function Card({
  label,
  value,
  sub,
  accent,
  delay,
}: {
  label: string;
  value: string | number;
  sub?: string;
  accent?: string;
  delay: number;
}) {
  return (
    <div
      className="animate-fade-up rounded-xl border border-line bg-surface p-4 shadow-card"
      style={{ animationDelay: `${delay}ms` }}
    >
      <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-fg-mute">{label}</p>
      <p className={`mt-1 font-display text-3xl font-bold tabular-nums ${accent ?? "text-fg"}`}>
        {value}
      </p>
      {sub && <p className="mt-0.5 text-xs text-fg-mute">{sub}</p>}
    </div>
  );
}

export function StatStrip({ events }: { events: StoredEvent[] }) {
  const stats = useMemo(() => {
    const bySev: Record<string, number> = {};
    const hosts = new Set<string>();
    const agents = new Set<string>();
    let latest = "";
    for (const e of events) {
      bySev[e.severity] = (bySev[e.severity] ?? 0) + 1;
      hosts.add(e.host);
      agents.add(e.agent_id);
      if (!latest || e.occurred_at > latest) latest = e.occurred_at;
    }
    const critical = (bySev.critical ?? 0) + (bySev.error ?? 0);
    return { total: events.length, critical, warning: bySev.warning ?? 0, hosts: hosts.size, agents: agents.size, latest };
  }, [events]);

  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
      <Card label="Events" value={stats.total} sub="in view" delay={0} />
      <Card
        label="Critical · Error"
        value={stats.critical}
        accent={stats.critical > 0 ? severityMeta("critical").text : "text-fg"}
        sub={stats.critical > 0 ? "needs attention" : "all clear"}
        delay={40}
      />
      <Card
        label="Warnings"
        value={stats.warning}
        accent={stats.warning > 0 ? severityMeta("warning").text : "text-fg"}
        delay={80}
      />
      <Card label="Hosts" value={stats.hosts} sub={`${stats.agents} agents`} delay={120} />
      <Card
        label="Last event"
        value={stats.latest ? relativeTime(stats.latest) : "—"}
        delay={160}
      />
    </div>
  );
}
