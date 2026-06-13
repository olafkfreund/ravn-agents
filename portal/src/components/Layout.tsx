import { useState } from "react";
import { useLocation, Outlet } from "react-router-dom";
import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";
import { SystemHealthBanner } from "./SystemHealthBanner";

const TITLES: Record<string, string> = {
  "/events": "Events",
  "/agents": "Agents",
  "/topology": "Topology",
};

export function Layout() {
  const [navOpen, setNavOpen] = useState(false);
  const { pathname } = useLocation();
  const title = TITLES[pathname] ?? "Ravn";

  return (
    <div className="flex h-full">
      <Sidebar open={navOpen} onClose={() => setNavOpen(false)} />
      <div className="flex min-w-0 flex-1 flex-col">
        <Topbar title={title} live onMenu={() => setNavOpen(true)} />
        {/* #149: self-observability banner — hidden when everything is nominal */}
        <SystemHealthBanner />
        <main className="flex-1 overflow-y-auto px-4 py-6 md:px-6 lg:px-8">
          <div className="mx-auto max-w-[1400px]">
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
}
