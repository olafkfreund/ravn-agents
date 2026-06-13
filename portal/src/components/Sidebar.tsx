import { NavLink } from "react-router-dom";

interface NavItem {
  to: string;
  label: string;
  glyph: string;
  soon?: boolean;
}

const NAV: NavItem[] = [
  { to: "/events", label: "Events", glyph: "◈" },
  { to: "/agents", label: "Agents", glyph: "❖" },
  { to: "/topology", label: "Topology", glyph: "⬡" },
  { to: "/remediations", label: "Remediations", glyph: "⟳" },
  { to: "/settings", label: "Settings", glyph: "⚙" },
];

export function Sidebar({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <>
      {/* Mobile backdrop */}
      {open && (
        <div
          className="fixed inset-0 z-30 bg-black/50 backdrop-blur-sm lg:hidden animate-fade-in"
          onClick={onClose}
          aria-hidden="true"
        />
      )}

      <aside
        className={`fixed inset-y-0 left-0 z-40 flex w-64 flex-col border-r border-line bg-surface
                    transition-transform duration-300 lg:static lg:z-0 lg:translate-x-0
                    ${open ? "translate-x-0" : "-translate-x-full"}`}
      >
        {/* Brand */}
        <div className="flex h-16 items-center gap-3 border-b border-line-soft px-5">
          <span className="grid h-9 w-9 place-items-center rounded-lg bg-accent/15 text-lg">
            🐦‍⬛
          </span>
          <div className="leading-tight">
            <div className="font-display text-lg font-extrabold tracking-tight">Ravn</div>
            <div className="font-mono text-[10px] uppercase tracking-[0.18em] text-fg-mute">
              operator console
            </div>
          </div>
        </div>

        {/* Nav */}
        <nav className="flex-1 space-y-1 px-3 py-4">
          <p className="px-3 pb-2 font-mono text-[10px] uppercase tracking-[0.18em] text-fg-mute">
            Fleet
          </p>
          {NAV.map((item) =>
            item.soon ? (
              <div
                key={item.to}
                className="flex cursor-not-allowed items-center gap-3 rounded-lg px-3 py-2 text-sm text-fg-mute/60"
                title="Coming soon"
              >
                <span className="w-4 text-center">{item.glyph}</span>
                <span>{item.label}</span>
                <span className="ml-auto rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-wide text-fg-mute">
                  soon
                </span>
              </div>
            ) : (
              <NavLink
                key={item.to}
                to={item.to}
                onClick={onClose}
                className={({ isActive }) =>
                  `group flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors ${
                    isActive
                      ? "bg-accent/12 text-fg"
                      : "text-fg-dim hover:bg-surface-2 hover:text-fg"
                  }`
                }
              >
                {({ isActive }) => (
                  <>
                    <span
                      className={`w-4 text-center transition-colors ${
                        isActive ? "text-accent" : "text-fg-mute group-hover:text-fg-dim"
                      }`}
                    >
                      {item.glyph}
                    </span>
                    <span className={isActive ? "font-semibold" : ""}>{item.label}</span>
                    {isActive && <span className="ml-auto h-4 w-1 rounded-full bg-accent" />}
                  </>
                )}
              </NavLink>
            ),
          )}
        </nav>

        {/* Footer */}
        <div className="border-t border-line-soft p-3">
          <a
            href="https://github.com/olafkfreund/ravn-agents"
            className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-fg-dim hover:bg-surface-2 hover:text-fg"
          >
            <span className="w-4 text-center text-fg-mute">↗</span>
            <span>GitHub</span>
          </a>
          <p className="px-3 pt-2 font-mono text-[10px] text-fg-mute">v0.1.0-dev · M1</p>
        </div>
      </aside>
    </>
  );
}
