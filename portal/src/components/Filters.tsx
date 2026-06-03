import { forwardRef } from "react";
import { SEVERITY_ORDER, severityMeta, type SeverityKey } from "../lib/format";

interface FiltersProps {
  search: string;
  onSearch: (v: string) => void;
  active: Set<SeverityKey>;
  onToggle: (k: SeverityKey) => void;
  onRefresh: () => void;
  refreshing: boolean;
}

export const Filters = forwardRef<HTMLInputElement, FiltersProps>(function Filters(
  { search, onSearch, active, onToggle, onRefresh, refreshing },
  ref,
) {
  return (
    <div className="flex flex-col gap-3 lg:flex-row lg:items-center">
      {/* Search */}
      <div className="relative flex-1">
        <span className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-mute">
          ⌕
        </span>
        <input
          ref={ref}
          value={search}
          onChange={(e) => onSearch(e.target.value)}
          placeholder="Search host or title…"
          className="w-full rounded-lg border border-line bg-surface py-2 pl-9 pr-12 text-sm text-fg
                     placeholder:text-fg-mute focus:border-accent focus-ring"
        />
        <span className="absolute right-3 top-1/2 hidden -translate-y-1/2 sm:block">
          <span className="kbd">/</span>
        </span>
      </div>

      {/* Severity chips */}
      <div className="flex flex-wrap items-center gap-1.5">
        {SEVERITY_ORDER.map((k) => {
          const m = severityMeta(k);
          const on = active.size === 0 || active.has(k);
          return (
            <button
              key={k}
              type="button"
              onClick={() => onToggle(k)}
              aria-pressed={active.has(k)}
              className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-all ${
                active.has(k)
                  ? `${m.tint} ${m.border} ${m.text}`
                  : "border-line text-fg-dim hover:border-fg-mute"
              } ${!on ? "opacity-40" : ""}`}
            >
              <span className="h-2 w-2 rounded-full" style={{ background: `rgb(var(--sev-${k}))` }} />
              {m.label}
            </button>
          );
        })}

        <button
          type="button"
          onClick={onRefresh}
          className="ml-1 grid h-8 w-8 place-items-center rounded-lg border border-line text-fg-dim hover:border-accent hover:text-fg"
          title="Refresh"
          aria-label="Refresh"
        >
          <span className={refreshing ? "inline-block animate-spin" : ""}>↻</span>
        </button>
      </div>
    </div>
  );
});
