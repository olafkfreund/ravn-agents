import { NavLink, Outlet } from "react-router-dom";
import { ThemeToggle } from "./ThemeToggle";

const navClass = ({ isActive }: { isActive: boolean }) =>
  `text-sm transition-colors hover:text-accent ${
    isActive ? "text-accent font-semibold" : "text-fg-soft"
  }`;

export function Layout() {
  return (
    <div className="min-h-full">
      <header className="sticky top-0 z-20 border-b border-border bg-bg/90 backdrop-blur">
        <div className="mx-auto flex max-w-6xl items-center gap-6 px-5 py-3">
          <a href="/" className="flex items-center gap-2 font-extrabold text-fg">
            <span aria-hidden="true">🐦‍⬛</span>
            <span>Ravn</span>
            <span className="text-muted font-normal">Portal</span>
          </a>
          <nav className="flex items-center gap-5">
            <NavLink to="/events" className={navClass}>
              Events
            </NavLink>
          </nav>
          <div className="ml-auto flex items-center gap-3">
            <a
              href="https://github.com/olafkfreund/ravn-agents"
              className="text-sm text-fg-soft hover:text-accent"
            >
              GitHub ↗
            </a>
            <ThemeToggle />
          </div>
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-5 py-8">
        <Outlet />
      </main>
    </div>
  );
}
