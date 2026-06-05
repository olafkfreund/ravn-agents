import { useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  approveRemediation,
  getMe,
  listRemediations,
  rejectRemediation,
  type ActionStatus,
  type RemediationRecord,
  type RiskTier,
} from "../lib/api";
import { absoluteTime, relativeTime } from "../lib/format";

const RISK: Record<RiskTier, { label: string; cls: string }> = {
  safe: { label: "safe", cls: "text-sev-notice border-sev-notice/30 bg-sev-notice/10" },
  guarded: { label: "guarded", cls: "text-sev-warning border-sev-warning/30 bg-sev-warning/10" },
  dangerous: { label: "dangerous", cls: "text-sev-critical border-sev-critical/40 bg-sev-critical/10" },
};

const STATUS: Record<ActionStatus, { label: string; cls: string }> = {
  succeeded: { label: "succeeded", cls: "text-sev-notice" },
  precondition_failed: { label: "precondition failed", cls: "text-sev-warning" },
  failed: { label: "failed", cls: "text-sev-error" },
  rejected: { label: "rejected", cls: "text-sev-warning" },
  frozen: { label: "frozen", cls: "text-sev-critical" },
};

function isPending(r: RemediationRecord): boolean {
  return r.decision.decision === "pending";
}

function decisionLabel(r: RemediationRecord): string {
  switch (r.decision.decision) {
    case "pending":
      return "Pending approval";
    case "approved":
      return r.decision.by.kind === "human" ? `Approved by ${r.decision.by.user}` : "Auto-approved";
    case "rejected":
      return `Rejected by ${r.decision.by}`;
  }
}

export function Remediations() {
  const qc = useQueryClient();
  const roleQ = useQuery({ queryKey: ["me"], queryFn: getMe });
  const remsQ = useQuery({
    queryKey: ["remediations"],
    queryFn: listRemediations,
    refetchInterval: 5_000,
  });

  const isAdmin = roleQ.data === "admin";
  const records = remsQ.data ?? [];
  const pending = useMemo(() => records.filter(isPending), [records]);
  const history = useMemo(() => records.filter((r) => !isPending(r)), [records]);

  const approve = useMutation({
    mutationFn: approveRemediation,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["remediations"] }),
  });
  const reject = useMutation({
    mutationFn: rejectRemediation,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["remediations"] }),
  });
  const busy = approve.isPending || reject.isPending;

  return (
    <div className="space-y-5">
      <div>
        <h2 className="font-display text-2xl font-bold tracking-tight">Remediations</h2>
        <p className="text-sm text-fg-mute">
          Proposed self-healing actions. Approve to sign and dispatch a command to the agent.
        </p>
      </div>

      {remsQ.isLoading && <p className="text-muted">Loading…</p>}
      {remsQ.isError && (
        <div className="rounded-xl border border-sev-error/40 bg-sev-error/8 p-6 text-center text-sev-error">
          Couldn’t reach the control plane.
        </div>
      )}

      {!isAdmin && roleQ.data && (
        <div className="rounded-lg border border-line bg-surface-2 px-3 py-2 text-sm text-fg-mute">
          You’re signed in as <span className="text-fg">viewer</span> — approving and rejecting
          requires the admin role.
        </div>
      )}

      {/* Pending queue */}
      <section className="space-y-3">
        <h3 className="font-mono text-[11px] uppercase tracking-[0.14em] text-fg-mute">
          Pending approval · {pending.length}
        </h3>
        {remsQ.data && pending.length === 0 && (
          <div className="rounded-xl border border-dashed border-line bg-surface p-10 text-center">
            <p className="text-3xl">✓</p>
            <p className="mt-2 font-display text-lg font-bold">Nothing awaiting approval</p>
            <p className="mt-1 text-sm text-fg-mute">
              Proposals appear here when a detected fault matches a remediation template.
            </p>
          </div>
        )}
        {pending.map((r) => {
          const p = r.proposal;
          const risk = RISK[p.risk_tier];
          return (
            <div key={p.id} className="rounded-xl border border-line bg-surface p-4 shadow-card">
              <div className="flex flex-wrap items-center gap-2">
                <span className={`rounded-full border px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider ${risk.cls}`}>
                  {risk.label}
                </span>
                <span className="font-semibold">{p.host}</span>
                <span className="font-mono text-[11px] text-fg-mute">
                  {p.template_id}@{p.template_version}
                </span>
                <span className="ml-auto font-mono text-[11px] text-fg-mute" title={absoluteTime(p.created_at)}>
                  {relativeTime(p.created_at)}
                </span>
              </div>

              <p className="mt-2 text-sm text-fg-dim">{p.rationale}</p>

              {Object.keys(p.params).length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {Object.entries(p.params).map(([k, v]) => (
                    <span key={k} className="rounded-full border border-line bg-surface-2 px-1.5 py-0.5 text-[11px]">
                      <span className="text-fg-mute">{k}:</span> <span className="text-accent-2">{v}</span>
                    </span>
                  ))}
                </div>
              )}

              <div className="mt-3 flex gap-2">
                <button
                  onClick={() => approve.mutate(p.id)}
                  disabled={!isAdmin || busy}
                  className="rounded-lg border border-sev-notice/40 bg-sev-notice/10 px-3 py-1.5 text-sm font-semibold text-sev-notice transition-colors hover:bg-sev-notice/20 focus-ring disabled:cursor-not-allowed disabled:opacity-40"
                >
                  Approve &amp; dispatch
                </button>
                <button
                  onClick={() => reject.mutate(p.id)}
                  disabled={!isAdmin || busy}
                  className="rounded-lg border border-line px-3 py-1.5 text-sm text-fg-dim transition-colors hover:border-fg-mute hover:text-fg focus-ring disabled:cursor-not-allowed disabled:opacity-40"
                >
                  Reject
                </button>
              </div>
            </div>
          );
        })}
      </section>

      {/* History / audit timeline */}
      {history.length > 0 && (
        <section className="space-y-3">
          <h3 className="font-mono text-[11px] uppercase tracking-[0.14em] text-fg-mute">
            History · {history.length}
          </h3>
          <div className="overflow-hidden rounded-xl border border-line">
            {history.map((r, i) => {
              const p = r.proposal;
              const result = r.result;
              const status = result ? STATUS[result.status] : null;
              return (
                <div
                  key={p.id}
                  className={`flex flex-wrap items-center gap-x-3 gap-y-1 bg-surface px-4 py-3 text-sm ${
                    i > 0 ? "border-t border-line-soft" : ""
                  }`}
                >
                  <span className="font-semibold">{p.host}</span>
                  <span className="font-mono text-[11px] text-fg-mute">{p.template_id}</span>
                  <span className="text-fg-dim">{decisionLabel(r)}</span>
                  {status ? (
                    <span className={`font-mono text-[11px] ${status.cls}`}>
                      → {status.label}
                      {result?.observed_state ? ` (${result.observed_state})` : ""}
                    </span>
                  ) : (
                    r.decision.decision === "approved" && (
                      <span className="font-mono text-[11px] text-fg-mute">→ dispatched, awaiting result</span>
                    )
                  )}
                  <span className="ml-auto font-mono text-[11px] text-fg-mute" title={absoluteTime(r.updated_at)}>
                    {relativeTime(r.updated_at)}
                  </span>
                </div>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
}
