import type { StoredEvent } from "../lib/api";
import { absoluteTime, relativeTime, severityMeta, severityVar, sourceLabel } from "../lib/format";

interface Props {
  events: StoredEvent[];
  selectedId?: string;
  onSelect: (e: StoredEvent) => void;
}

function SevPill({ severity }: { severity: string }) {
  const m = severityMeta(severity);
  return (
    <span className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs ${m.tint} ${m.border} ${m.text}`}>
      <span className="h-1.5 w-1.5 rounded-full" style={{ background: severityVar(severity) }} />
      {m.label}
    </span>
  );
}

export function EventsTable({ events, selectedId, onSelect }: Props) {
  return (
    <>
      {/* Desktop / tablet table */}
      <div className="hidden overflow-hidden rounded-xl border border-line bg-surface shadow-card md:block">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr className="border-b border-line text-left font-mono text-[10px] uppercase tracking-[0.14em] text-fg-mute">
              <th className="w-1" />
              <th className="px-4 py-2.5 font-medium">Severity</th>
              <th className="px-4 py-2.5 font-medium">Event</th>
              <th className="px-4 py-2.5 font-medium">Source</th>
              <th className="px-4 py-2.5 font-medium">Host</th>
              <th className="px-4 py-2.5 text-right font-medium">When</th>
            </tr>
          </thead>
          <tbody>
            {events.map((e, i) => (
              <tr
                key={e.id}
                onClick={() => onSelect(e)}
                style={{ animationDelay: `${Math.min(i * 22, 400)}ms` }}
                className={`group animate-fade-up cursor-pointer border-b border-line-soft transition-colors last:border-0 ${
                  selectedId === e.id ? "bg-accent/8" : "hover:bg-surface-2"
                }`}
              >
                <td className="p-0">
                  <span className="block h-full w-1" style={{ background: severityVar(e.severity) }} />
                </td>
                <td className="px-4 py-3 align-top">
                  <SevPill severity={e.severity} />
                </td>
                <td className="px-4 py-3 align-top">
                  <span className="font-medium text-fg group-hover:text-accent">{e.title}</span>
                </td>
                <td className="px-4 py-3 align-top">
                  <span className="font-mono text-xs text-fg-dim">{sourceLabel(e.source)}</span>
                </td>
                <td className="px-4 py-3 align-top font-mono text-xs text-fg-dim">{e.host}</td>
                <td className="px-4 py-3 text-right align-top">
                  <span className="font-mono text-xs text-fg-mute" title={absoluteTime(e.occurred_at)}>
                    {relativeTime(e.occurred_at)}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Mobile cards */}
      <div className="space-y-2 md:hidden">
        {events.map((e, i) => (
          <button
            key={e.id}
            onClick={() => onSelect(e)}
            style={{ animationDelay: `${Math.min(i * 22, 400)}ms` }}
            className="flex w-full animate-fade-up items-stretch gap-3 overflow-hidden rounded-xl border border-line bg-surface text-left shadow-card"
          >
            <span className="w-1 shrink-0" style={{ background: severityVar(e.severity) }} />
            <div className="min-w-0 flex-1 py-3 pr-3">
              <div className="flex items-center justify-between gap-2">
                <SevPill severity={e.severity} />
                <span className="font-mono text-[11px] text-fg-mute">{relativeTime(e.occurred_at)}</span>
              </div>
              <p className="mt-1.5 font-medium text-fg">{e.title}</p>
              <p className="mt-0.5 font-mono text-xs text-fg-mute">
                {sourceLabel(e.source)} · {e.host}
              </p>
            </div>
          </button>
        ))}
      </div>
    </>
  );
}
