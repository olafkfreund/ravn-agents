import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { setLabels, type Agent, type StoredEvent } from "../lib/api";
import { absoluteTime, relativeTime, severityVar, sourceLabel, statusMeta } from "../lib/format";

interface Entry {
  key: string;
  value: string;
}

export function AgentDrawer({
  agent,
  events,
  onClose,
}: {
  agent: Agent | null;
  events: StoredEvent[];
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [entries, setEntries] = useState<Entry[]>([]);

  useEffect(() => {
    setEntries(Object.entries(agent?.labels ?? {}).map(([key, value]) => ({ key, value })));
  }, [agent]);

  useEffect(() => {
    if (!agent) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [agent, onClose]);

  const save = useMutation({
    mutationFn: (vars: { id: string; labels: Record<string, string> }) => setLabels(vars.id, vars.labels),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["agents"] });
      qc.invalidateQueries({ queryKey: ["categories"] });
      qc.invalidateQueries({ queryKey: ["topology"] });
    },
  });

  if (!agent) return null;
  const s = statusMeta(agent.status);
  const recent = events.filter((e) => e.agent_id === agent.agent_id).slice(0, 15);

  const onSave = () => {
    const labels: Record<string, string> = {};
    for (const e of entries) {
      const k = e.key.trim();
      if (k) labels[k] = e.value.trim();
    }
    save.mutate({ id: agent.agent_id, labels });
  };

  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm animate-fade-in" onClick={onClose} />
      <aside
        className="absolute inset-y-0 right-0 flex w-full animate-slide-in flex-col border-l border-line bg-surface shadow-drawer sm:max-w-xl"
        role="dialog"
        aria-modal="true"
      >
        <div className="flex items-start gap-3 border-b border-line p-5">
          <span className="mt-1.5 h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: s.dot }} />
          <div className="min-w-0 flex-1">
            <div className={`text-xs font-semibold uppercase tracking-wide ${s.text}`}>{s.label}</div>
            <h2 className="mt-0.5 font-display text-xl font-bold leading-snug">{agent.host}</h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="grid h-8 w-8 shrink-0 place-items-center rounded-lg border border-line text-fg-dim hover:text-fg"
          >
            ✕
          </button>
        </div>

        <div className="flex-1 space-y-6 overflow-y-auto p-5">
          <dl className="grid grid-cols-2 gap-4">
            <div>
              <dt className="font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">First seen</dt>
              <dd className="mt-0.5 font-mono text-sm text-fg-dim">{absoluteTime(agent.first_seen)}</dd>
            </div>
            <div>
              <dt className="font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">Last seen</dt>
              <dd className="mt-0.5 font-mono text-sm text-fg-dim">{relativeTime(agent.last_seen)}</dd>
            </div>
            <div className="col-span-2">
              <dt className="font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">Agent ID</dt>
              <dd className="mt-0.5 break-all font-mono text-sm text-fg-dim">{agent.agent_id}</dd>
            </div>
          </dl>

          {/* Editable labels (categories) */}
          <div>
            <div className="mb-2 flex items-center justify-between">
              <p className="font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">Labels / categories</p>
              {save.isSuccess && !save.isPending && <span className="text-xs text-sev-notice">saved ✓</span>}
              {save.isError && <span className="text-xs text-sev-error">save failed</span>}
            </div>
            <div className="space-y-2">
              {entries.map((e, i) => (
                <div key={i} className="flex items-center gap-2">
                  <input
                    value={e.key}
                    onChange={(ev) =>
                      setEntries((p) => p.map((x, j) => (j === i ? { ...x, key: ev.target.value } : x)))
                    }
                    placeholder="key (e.g. env)"
                    className="w-1/3 rounded-md border border-line bg-bg px-2 py-1 font-mono text-xs text-fg focus:border-accent focus-ring"
                  />
                  <span className="text-fg-mute">:</span>
                  <input
                    value={e.value}
                    onChange={(ev) =>
                      setEntries((p) => p.map((x, j) => (j === i ? { ...x, value: ev.target.value } : x)))
                    }
                    placeholder="value (e.g. prod)"
                    className="flex-1 rounded-md border border-line bg-bg px-2 py-1 font-mono text-xs text-fg focus:border-accent focus-ring"
                  />
                  <button
                    type="button"
                    onClick={() => setEntries((p) => p.filter((_, j) => j !== i))}
                    aria-label="Remove label"
                    className="grid h-7 w-7 shrink-0 place-items-center rounded-md border border-line text-fg-mute hover:border-sev-error hover:text-sev-error"
                  >
                    ✕
                  </button>
                </div>
              ))}
            </div>
            <div className="mt-3 flex items-center gap-2">
              <button
                type="button"
                onClick={() => setEntries((p) => [...p, { key: "", value: "" }])}
                className="rounded-md border border-line px-2.5 py-1 text-xs text-fg-dim hover:border-accent hover:text-fg"
              >
                + Add label
              </button>
              <button
                type="button"
                onClick={onSave}
                disabled={save.isPending}
                className="rounded-md border border-accent bg-accent/15 px-3 py-1 text-xs font-semibold text-accent hover:bg-accent/25 disabled:opacity-50"
              >
                {save.isPending ? "Saving…" : "Save"}
              </button>
            </div>
            <p className="mt-1.5 text-[11px] text-fg-mute">
              Labels are the grouping dimensions used by the Topology view.
            </p>
          </div>

          <div>
            <p className="mb-2 font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">
              Recent events ({recent.length})
            </p>
            {recent.length === 0 ? (
              <p className="text-sm text-fg-mute">No events from this agent in view.</p>
            ) : (
              <ul className="space-y-1.5">
                {recent.map((e) => (
                  <li key={e.id} className="flex items-start gap-2.5 rounded-lg border border-line-soft bg-bg p-2.5">
                    <span className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full" style={{ background: severityVar(e.severity) }} />
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm text-fg">{e.title}</p>
                      <p className="font-mono text-[11px] text-fg-mute">
                        {sourceLabel(e.source)} · {relativeTime(e.occurred_at)}
                      </p>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </aside>
    </div>
  );
}
