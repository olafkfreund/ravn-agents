import { useState, useEffect } from "react";
import { useLocation, Outlet } from "react-router-dom";
import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";
import { SystemHealthBanner } from "./SystemHealthBanner";
import { useEventStream } from "../lib/useEventStream";

const TITLES: Record<string, string> = {
  "/events": "Events",
  "/agents": "Agents",
  "/topology": "Topology",
};

export function Layout() {
  const [navOpen, setNavOpen] = useState(false);
  const { pathname } = useLocation();
  const title = TITLES[pathname] ?? "Ravn";

  // Subscribe to live events for global desktop notifications
  const liveEvents = useEventStream(true);
  const [lastNotifiedId, setLastNotifiedId] = useState<string | null>(null);

  // Request notification permissions on mount
  useEffect(() => {
    if (typeof window !== "undefined" && "Notification" in window) {
      if (Notification.permission === "default") {
        Notification.requestPermission().catch(() => {});
      }
    }
  }, []);

  // Show desktop notification when a critical, error, or warning event arrives
  useEffect(() => {
    if (liveEvents.length === 0) return;
    const latest = liveEvents[0];
    if (latest.id === lastNotifiedId) return;
    setLastNotifiedId(latest.id);

    const severity = (latest.severity || "").toLowerCase();
    if (severity === "critical" || severity === "error" || severity === "warning") {
      if (typeof window !== "undefined" && "Notification" in window && Notification.permission === "granted") {
        new Notification(`Ravn Alert [${severity.toUpperCase()}]: ${latest.host}`, {
          body: latest.title || "A system alert has been detected.",
        });
      }
    }
  }, [liveEvents, lastNotifiedId]);

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
