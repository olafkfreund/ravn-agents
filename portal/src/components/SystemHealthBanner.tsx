/**
 * SystemHealthBanner — self-observability surface for issue #149.
 *
 * Polls /api/system/health every 15 s and shows a compact banner when
 * anything notable is happening: kill-switch active, circuit-breaker trips,
 * inference not configured / timing out, command-channel errors.
 *
 * Rendered transparently (null) when the endpoint is absent (older control
 * planes) or everything is nominal — zero noise in the happy path.
 */
import { useQuery } from "@tanstack/react-query";
import { getSystemHealth, type SystemHealth } from "../lib/api";

function hasConcern(h: SystemHealth): boolean {
  return (
    h.kill_switch_active ||
    h.circuit_breaker_trips_total > 0 ||
    !h.inference_configured ||
    h.inference_timeouts_total > 0 ||
    h.command_channel_errors_total > 0
  );
}

export function SystemHealthBanner() {
  const { data } = useQuery({
    queryKey: ["system-health"],
    queryFn: getSystemHealth,
    refetchInterval: 15_000,
    // Silently degrade — missing endpoint should not surface an error.
    retry: false,
  });

  if (!data || !hasConcern(data)) return null;

  const items: { label: string; value: string; warn: boolean }[] = [];

  if (data.kill_switch_active) {
    items.push({
      label: "kill-switch",
      value: "auto-remediation DISABLED",
      warn: true,
    });
  }

  if (data.circuit_breaker_trips_total > 0) {
    items.push({
      label: "breaker trips",
      value: String(data.circuit_breaker_trips_total),
      warn: true,
    });
  }

  if (!data.inference_configured) {
    items.push({
      label: "inference",
      value: "not configured",
      warn: false,
    });
  } else if (data.inference_timeouts_total > 0) {
    items.push({
      label: "inference timeouts",
      value: String(data.inference_timeouts_total),
      warn: true,
    });
  }

  if (data.command_channel_errors_total > 0) {
    items.push({
      label: "cmd-channel errors",
      value: String(data.command_channel_errors_total),
      warn: true,
    });
  }

  const hasWarning = items.some((i) => i.warn);

  return (
    <div
      className={`flex flex-wrap items-center gap-x-4 gap-y-1 border-b px-4 py-2 text-[11px] font-mono ${
        hasWarning
          ? "border-sev-warning/30 bg-sev-warning/8 text-sev-warning"
          : "border-line-soft bg-surface-2 text-fg-mute"
      }`}
      role="status"
      aria-label="System health"
    >
      <span className="font-semibold uppercase tracking-wider">
        {hasWarning ? "! system" : "system"}
      </span>
      {items.map((item) => (
        <span key={item.label} className="flex items-center gap-1">
          <span className="text-fg-mute">{item.label}:</span>
          <span className={item.warn ? "text-sev-warning font-semibold" : ""}>
            {item.value}
          </span>
        </span>
      ))}
    </div>
  );
}
