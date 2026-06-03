import { useEffect } from "react";
import type { Agent, StoredEvent } from "../lib/api";
import { absoluteTime, relativeTime, severityVar, sourceLabel, statusMeta } from "../lib/format";

export function AgentDrawer({
  agent,
  events,
  onClose,
}: {
  agent: Agent | null;
  events: StoredEvent[];
  onClose: () => void;
}) {
  useEffect(() => {
    if (!agent) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [agent, onClose]);

  if (!agent) return null;
  const s = statusMeta(agent.status);
  const recent = events.filter((e) => e.agent_id === agent.agent_id).slice(0, 15);
  const labels = Object.entries(agent.labels ?? {});

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

          <div>
            <p className="mb-1.5 font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">Labels</p>
            {labels.length > 0 ? (
              <div className="flex flex-wrap gap-1.5">
                {labels.map(([k, v]) => (
                  <span key={k} className="rounded-full border border-line bg-surface-2 px-2 py-0.5 text-xs">
                    <span className="text-fg-mute">{k}:</span> <span className="text-accent-2">{v}</span>
                  </span>
                ))}
              </div>
            ) : (
              <p className="text-sm text-fg-mute">No labels. Add some to group this agent in topology.</p>
            )}
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
