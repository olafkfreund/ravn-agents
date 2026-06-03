import { useQuery } from "@tanstack/react-query";
import { listEvents } from "../lib/api";
import { Badge, severityTone } from "../components/ui/Badge";

function formatTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export function Events() {
  const { data, isLoading, isError, refetch, isFetching } = useQuery({
    queryKey: ["events"],
    queryFn: () => listEvents(100),
    refetchInterval: 10_000,
  });

  return (
    <section>
      <div className="mb-5 flex items-baseline justify-between gap-4">
        <div>
          <h1 className="text-2xl font-bold">Events</h1>
          <p className="text-fg-soft">Recent detection events across the fleet.</p>
        </div>
        <button
          type="button"
          onClick={() => refetch()}
          className="rounded-md border border-border bg-bg-elev px-3 py-1 text-sm hover:border-accent"
        >
          {isFetching ? "Refreshing…" : "Refresh"}
        </button>
      </div>

      {isLoading && <p className="text-muted">Loading…</p>}
      {isError && (
        <p className="text-red">
          Couldn’t reach the control plane. Is <code>ravn-server</code> running and proxied?
        </p>
      )}

      {data && data.length === 0 && (
        <div className="rounded-lg border border-border bg-bg-soft p-8 text-center text-muted">
          No events yet. Start an agent and they’ll appear here.
        </div>
      )}

      {data && data.length > 0 && (
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="bg-bg-elev text-left">
                <th className="px-3 py-2 font-semibold">Time</th>
                <th className="px-3 py-2 font-semibold">Severity</th>
                <th className="px-3 py-2 font-semibold">Source</th>
                <th className="px-3 py-2 font-semibold">Host</th>
                <th className="px-3 py-2 font-semibold">Title</th>
              </tr>
            </thead>
            <tbody>
              {data.map((e) => (
                <tr key={e.id} className="border-t border-border align-top hover:bg-bg-soft">
                  <td className="whitespace-nowrap px-3 py-2 font-mono text-xs text-muted">
                    {formatTime(e.occurred_at)}
                  </td>
                  <td className="px-3 py-2">
                    <Badge tone={severityTone(e.severity)}>{e.severity}</Badge>
                  </td>
                  <td className="px-3 py-2 font-mono text-xs">{e.source}</td>
                  <td className="px-3 py-2">{e.host}</td>
                  <td className="px-3 py-2">{e.title}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
