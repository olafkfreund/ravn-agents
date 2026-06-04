import { ThemeToggle } from "./ThemeToggle";
import { useAuth } from "../lib/AuthContext";

interface TopbarProps {
  title: string;
  onMenu: () => void;
  live: boolean;
}

export function Topbar({ title, onMenu, live }: TopbarProps) {
  const { required, user, role, logout } = useAuth();
  return (
    <header className="sticky top-0 z-20 flex h-16 items-center gap-3 border-b border-line bg-bg/80 px-4 backdrop-blur-md md:px-6">
      <button
        type="button"
        onClick={onMenu}
        aria-label="Open navigation"
        className="grid h-9 w-9 place-items-center rounded-lg border border-line text-fg-dim hover:text-fg lg:hidden"
      >
        <span className="text-lg leading-none">≡</span>
      </button>

      <h1 className="font-display text-xl font-bold tracking-tight">{title}</h1>

      <div className="ml-auto flex items-center gap-2 sm:gap-3">
        <span
          className="flex items-center gap-2 rounded-full border border-line bg-surface px-3 py-1 text-xs text-fg-dim"
          title={live ? "Live — auto-refreshing" : "Disconnected"}
        >
          <span className="relative flex h-2 w-2">
            <span
              className={`absolute inline-flex h-2 w-2 rounded-full ${
                live ? "bg-sev-notice animate-pulse-ring" : "bg-sev-error"
              }`}
            />
            <span
              className={`inline-flex h-2 w-2 rounded-full ${live ? "bg-sev-notice" : "bg-sev-error"}`}
            />
          </span>
          <span className="hidden sm:inline font-mono uppercase tracking-wider">
            {live ? "live" : "offline"}
          </span>
        </span>
        <ThemeToggle />

        {required && user && (
          <div className="flex items-center gap-2">
            <span
              className="hidden items-center gap-1.5 rounded-full border border-line bg-surface px-3 py-1 text-xs text-fg-dim sm:flex"
              title={`Signed in as ${user} (${role})`}
            >
              <span className="max-w-[12rem] truncate">{user}</span>
              <span className="rounded bg-sev-notice/15 px-1.5 py-0.5 font-mono uppercase tracking-wider text-sev-notice">
                {role}
              </span>
            </span>
            <button
              type="button"
              onClick={logout}
              className="rounded-lg border border-line px-3 py-1 text-xs text-fg-dim hover:text-fg"
            >
              Sign out
            </button>
          </div>
        )}
      </div>
    </header>
  );
}
