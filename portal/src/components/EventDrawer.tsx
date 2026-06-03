import { useEffect } from "react";
import type { StoredEvent } from "../lib/api";
import { absoluteTime, severityMeta, severityVar, sourceLabel } from "../lib/format";

function Field({ label, value, mono = true }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <dt className="font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">{label}</dt>
      <dd className={`mt-0.5 break-words text-sm text-fg-dim ${mono ? "font-mono" : ""}`}>{value}</dd>
    </div>
  );
}

export function EventDrawer({ event, onClose }: { event: StoredEvent | null; onClose: () => void }) {
  useEffect(() => {
    if (!event) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [event, onClose]);

  if (!event) return null;
  const m = severityMeta(event.severity);
  const expl = (event.explanation ?? null) as unknown as {
    text?: string;
    suggested_check?: string;
    model?: string;
  } | null;

  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm animate-fade-in" onClick={onClose} />
      <aside
        className="absolute inset-y-0 right-0 flex w-full animate-slide-in flex-col border-l border-line bg-surface shadow-drawer sm:max-w-xl"
        role="dialog"
        aria-modal="true"
      >
        {/* Header */}
        <div className="flex items-start gap-3 border-b border-line p-5" style={{ borderTopColor: severityVar(event.severity) }}>
          <span className="mt-1 h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: severityVar(event.severity) }} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className={`text-xs font-semibold uppercase tracking-wide ${m.text}`}>{m.label}</span>
              <span className="font-mono text-[11px] text-fg-mute">· {sourceLabel(event.source)}</span>
            </div>
            <h2 className="mt-1 font-display text-xl font-bold leading-snug">{event.title}</h2>
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

        {/* Body */}
        <div className="flex-1 space-y-6 overflow-y-auto p-5">
          {/* Explanation — Ravn's differentiator */}
          <section className={`rounded-xl border p-4 ${expl?.text ? "border-accent-2/40 bg-accent-2/8" : "border-line bg-surface-2"}`}>
            <div className="mb-2 flex items-center gap-2">
              <span className="text-accent-2">✦</span>
              <h3 className="font-mono text-[11px] uppercase tracking-[0.16em] text-fg-dim">
                Explanation
              </h3>
            </div>
            {expl?.text ? (
              <>
                <p className="text-sm leading-relaxed text-fg">{expl.text}</p>
                {expl.suggested_check && (
                  <div className="mt-3">
                    <p className="font-mono text-[10px] uppercase tracking-wider text-fg-mute">Suggested check</p>
                    <pre className="mt-1 overflow-x-auto rounded-lg border border-line bg-bg p-3 font-mono text-xs text-accent-2">
{expl.suggested_check}</pre>
                  </div>
                )}
                {expl.model && <p className="mt-2 font-mono text-[10px] text-fg-mute">via {expl.model}</p>}
              </>
            ) : (
              <p className="text-sm leading-relaxed text-fg-mute">
                No explanation yet. Local inference attaches a plain-language summary and a suggested
                check once the model has processed this event — detection never waits on it.
              </p>
            )}
          </section>

          {/* Metadata */}
          <dl className="grid grid-cols-2 gap-4">
            <Field label="Host" value={event.host} />
            <Field label="Source" value={sourceLabel(event.source)} />
            <Field label="Occurred" value={absoluteTime(event.occurred_at)} />
            <Field label="Received" value={absoluteTime(event.received_at)} />
            <Field label="Agent" value={event.agent_id} />
            <Field label="Event ID" value={event.id} />
          </dl>

          {event.category_hints.length > 0 && (
            <div>
              <p className="mb-1.5 font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">Categories</p>
              <div className="flex flex-wrap gap-1.5">
                {event.category_hints.map((c) => (
                  <span key={c} className="rounded-full border border-line bg-surface-2 px-2 py-0.5 text-xs text-fg-dim">
                    {c}
                  </span>
                ))}
              </div>
            </div>
          )}

          {/* Raw payload */}
          <div>
            <p className="mb-1.5 font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">Payload</p>
            <pre className="overflow-x-auto rounded-lg border border-line bg-bg p-3 font-mono text-xs leading-relaxed text-fg-dim">
{JSON.stringify(event.payload, null, 2)}</pre>
          </div>
        </div>
      </aside>
    </div>
  );
}
