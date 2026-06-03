import { useEffect, useState } from "react";
import type { StoredEvent } from "./api";

/**
 * Subscribe to the live event WebSocket (#29). While `enabled`, new events are
 * prepended (capped). Returns the buffer; the caller merges with the initial
 * query and toggles `enabled` to pause.
 */
export function useEventStream(enabled: boolean): StoredEvent[] {
  const [events, setEvents] = useState<StoredEvent[]>([]);

  useEffect(() => {
    if (!enabled) return;
    const proto = window.location.protocol === "https:" ? "wss" : "ws";
    const url = `${proto}://${window.location.host}/ws/events`;
    let ws: WebSocket | null = null;
    let closed = false;

    try {
      ws = new WebSocket(url);
      ws.onmessage = (e) => {
        try {
          const ev = JSON.parse(e.data) as StoredEvent;
          setEvents((prev) => [ev, ...prev].slice(0, 200));
        } catch {
          /* ignore malformed */
        }
      };
    } catch {
      /* ignore connect failure; polling still covers it */
    }

    return () => {
      closed = true;
      ws?.close();
      void closed;
    };
  }, [enabled]);

  return events;
}
