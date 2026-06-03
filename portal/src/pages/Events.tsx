import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { listEvents, type StoredEvent } from "../lib/api";
import { type SeverityKey } from "../lib/format";
import { useEventStream } from "../lib/useEventStream";
import { StatStrip } from "../components/StatStrip";
import { Filters } from "../components/Filters";
import { EventsTable } from "../components/EventsTable";
import { EventDrawer } from "../components/EventDrawer";

export function Events() {
  const [search, setSearch] = useState("");
  const [active, setActive] = useState<Set<SeverityKey>>(new Set());
  const [selected, setSelected] = useState<StoredEvent | null>(null);
  const [paused, setPaused] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

  const live = useEventStream(!paused);

  const { data, isLoading, isError, refetch, isFetching } = useQuery({
    queryKey: ["events"],
    queryFn: () => listEvents(200),
    refetchInterval: 10_000,
  });

  // `/` focuses search (unless already typing).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const typing = el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
      if (e.key === "/" && !typing) {
        e.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const toggle = (k: SeverityKey) =>
    setActive((prev) => {
      const next = new Set(prev);
      next.has(k) ? next.delete(k) : next.add(k);
      return next;
    });

  // Merge the live WebSocket stream with the polled query, newest-first, deduped.
  const events = useMemo(() => {
    const byId = new Map<string, StoredEvent>();
    for (const e of [...live, ...(data ?? [])]) byId.set(e.id, e);
    return [...byId.values()].sort((a, b) => (a.occurred_at < b.occurred_at ? 1 : -1));
  }, [live, data]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return events.filter((e) => {
      if (active.size > 0 && !active.has(e.severity as SeverityKey)) return false;
      if (q && !(e.title.toLowerCase().includes(q) || e.host.toLowerCase().includes(q))) return false;
      return true;
    });
  }, [events, active, search]);

  return (
    <div className="space-y-5">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="font-display text-2xl font-bold tracking-tight">Fleet overview</h2>
          <p className="text-sm text-fg-mute">
            Deterministic detection across your servers — newest first, streaming live.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setPaused((p) => !p)}
          className={`flex shrink-0 items-center gap-2 rounded-full border px-3 py-1.5 text-xs transition-colors ${
            paused ? "border-line text-fg-dim hover:border-accent" : "border-sev-notice/40 bg-sev-notice/10 text-sev-notice"
          }`}
          title={paused ? "Resume live stream" : "Pause live stream"}
        >
          <span className={`h-2 w-2 rounded-full ${paused ? "bg-fg-mute" : "bg-sev-notice animate-pulse-ring"}`} />
          {paused ? "Paused" : "Live"}
        </button>
      </div>

      <StatStrip events={filtered} />

      <Filters
        ref={searchRef}
        search={search}
        onSearch={setSearch}
        active={active}
        onToggle={toggle}
        onRefresh={() => refetch()}
        refreshing={isFetching}
      />

      {isLoading && <TableSkeleton />}

      {isError && (
        <div className="rounded-xl border border-sev-error/40 bg-sev-error/8 p-6 text-center">
          <p className="font-display text-lg font-bold text-sev-error">Can't reach the control plane</p>
          <p className="mt-1 text-sm text-fg-dim">
            Is <code className="font-mono">ravn-server</code> running and proxied? Retrying every 10s.
          </p>
        </div>
      )}

      {!isLoading && !isError && filtered.length === 0 && (
        <div className="rounded-xl border border-dashed border-line bg-surface p-12 text-center">
          <p className="text-3xl">🐦‍⬛</p>
          <p className="mt-2 font-display text-lg font-bold">
            {events.length === 0 ? "Nothing to report" : "No matching events"}
          </p>
          <p className="mt-1 text-sm text-fg-mute">
            {events.length === 0
              ? "Agents are quiet. New detections land here in real time."
              : "Try clearing a filter or the search."}
          </p>
        </div>
      )}

      {filtered.length > 0 && (
        <EventsTable events={filtered} selectedId={selected?.id} onSelect={setSelected} />
      )}

      <EventDrawer event={selected} onClose={() => setSelected(null)} />
    </div>
  );
}

function TableSkeleton() {
  return (
    <div className="overflow-hidden rounded-xl border border-line bg-surface">
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="flex items-center gap-4 border-b border-line-soft px-4 py-3.5 last:border-0">
          <div className="h-5 w-16 animate-pulse rounded-full bg-elev" />
          <div className="h-4 flex-1 animate-pulse rounded bg-elev" style={{ maxWidth: `${40 + ((i * 13) % 40)}%` }} />
          <div className="h-4 w-20 animate-pulse rounded bg-elev" />
        </div>
      ))}
    </div>
  );
}
