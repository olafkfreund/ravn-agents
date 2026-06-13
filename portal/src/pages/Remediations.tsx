import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  approveRemediation,
  getMe,
  listRemediations,
  rejectRemediation,
  remediationErrorMessage,
  type RemediationRecord,
  type RiskTier,
} from "../lib/api";
import { absoluteTime, relativeTime } from "../lib/format";

const RISK: Record<RiskTier, { label: string; cls: string }> = {
  safe: { label: "safe", cls: "text-sev-notice border-sev-notice/30 bg-sev-notice/10" },
  guarded: { label: "guarded", cls: "text-sev-warning border-sev-warning/30 bg-sev-warning/10" },
  dangerous: { label: "dangerous", cls: "text-sev-critical border-sev-critical/40 bg-sev-critical/10" },
};

function isPending(r: RemediationRecord): boolean {
  return r.decision.decision === "pending";
}

function ApprovalBadge({ record }: { record: RemediationRecord }) {
  const dec = record.decision;
  if (dec.decision === "rejected") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full border border-sev-error/45 bg-sev-error/10 px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-sev-error">
        <svg className="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M18.36 5.64a9 9 0 11-12.73 12.73 9 9 0 0112.73-12.73zM6 18L18 6" />
        </svg>
        Rejected
      </span>
    );
  }
  
  if (dec.decision === "approved") {
    if (dec.by.kind === "policy_auto") {
      return (
        <span className="inline-flex items-center gap-1 rounded-full border border-accent-2/30 bg-accent-2/10 px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-accent-2">
          <svg className="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
          </svg>
          Auto-Corrected
        </span>
      );
    } else {
      return (
        <span className="inline-flex items-center gap-1 rounded-full border border-sev-info/30 bg-sev-info/10 px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-sev-info">
          <svg className="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z" />
          </svg>
          Approved: {dec.by.user}
        </span>
      );
    }
  }

  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-sev-warning/30 bg-sev-warning/10 px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-sev-warning">
      Pending
    </span>
  );
}

function ExecutionStatus({ record }: { record: RemediationRecord }) {
  const result = record.result;
  const dec = record.decision;

  if (dec.decision === "rejected") {
    return (
      <div className="mt-3 rounded-lg border border-line-soft bg-surface-2 px-3 py-2 text-xs text-fg-dim">
        <span className="font-semibold text-sev-error">Rejection Details:</span>{" "}
        Rejected by <span className="font-semibold">{dec.by}</span>
        {dec.reason && <> with reason: <span className="italic text-fg font-mono">"{dec.reason}"</span></>}
        {" · "}
        <span className="text-fg-mute font-mono">{relativeTime(dec.at)}</span>
      </div>
    );
  }

  if (!result) {
    if (dec.decision === "approved") {
      return (
        <div className="mt-3 flex items-center gap-2 rounded-lg border border-sev-warning/20 bg-sev-warning/5 px-3 py-2 text-xs text-sev-warning">
          <span className="relative flex h-2 w-2">
            <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-sev-warning opacity-75"></span>
            <span className="relative inline-flex rounded-full h-2 w-2 bg-sev-warning"></span>
          </span>
          <span className="font-mono">Dispatched to agent, awaiting execution results...</span>
        </div>
      );
    }
    return null;
  }

  switch (result.status) {
    case "succeeded":
      return (
        <div className="mt-3 rounded-lg border border-sev-notice/35 bg-sev-notice/5 px-3 py-2 text-xs text-fg-dim">
          <div className="flex items-center gap-1.5 text-sev-notice font-semibold">
            <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
            </svg>
            Corrected Successfully
          </div>
          {result.observed_state && (
            <div className="mt-1 font-mono text-[11px] text-fg-mute">
              Observed State: <span className="text-fg font-semibold">{result.observed_state}</span>
            </div>
          )}
          {result.detail && <p className="mt-1 text-fg-mute">{result.detail}</p>}
        </div>
      );
    case "failed":
      return (
        <div className="mt-3 rounded-lg border border-sev-error/35 bg-sev-error/5 px-3 py-2 text-xs text-fg-dim">
          <div className="flex items-center gap-1.5 text-sev-error font-semibold">
            <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
            Execution Failed
          </div>
          {result.detail && <p className="mt-1 text-sev-error font-mono bg-sev-error/5 p-1 rounded border border-sev-error/10">{result.detail}</p>}
        </div>
      );
    case "precondition_failed":
      return (
        <div className="mt-3 rounded-lg border border-sev-warning/35 bg-sev-warning/5 px-3 py-2 text-xs text-fg-dim">
          <div className="flex items-center gap-1.5 text-sev-warning font-semibold">
            <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
            Precondition Failed (Skipped)
          </div>
          {result.detail && <p className="mt-1 text-fg-mute">{result.detail}</p>}
        </div>
      );
    case "frozen":
      return (
        <div className="mt-3 rounded-lg border border-sev-critical/35 bg-sev-critical/10 px-3 py-2 text-xs text-fg animate-pulse">
          <div className="flex items-center gap-1.5 text-sev-critical font-bold uppercase tracking-wide">
            <svg className="h-4 w-4 animate-bounce" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
            FROZEN: Rollback Failed
          </div>
          <p className="mt-1 text-sev-critical font-semibold">Self-healing rollback failed. Pod or host is in an inconsistent state. Escalate immediately to human operator!</p>
          {result.detail && <p className="mt-1.5 text-xs font-mono bg-sev-critical/5 p-1 rounded border border-sev-critical/20 text-fg-dim">{result.detail}</p>}
        </div>
      );
    default:
      return null;
  }
}

export function Remediations() {
  const qc = useQueryClient();
  const roleQ = useQuery({ queryKey: ["me"], queryFn: getMe });
  const remsQ = useQuery({
    queryKey: ["remediations"],
    queryFn: listRemediations,
    refetchInterval: 5_000,
    retry: 1,
  });

  const [historyFilter, setHistoryFilter] = useState<"all" | "auto" | "manual" | "rejected">("all");

  const isAdmin = roleQ.data === "admin";
  const records = remsQ.data ?? [];
  const pending = useMemo(() => records.filter(isPending), [records]);
  const history = useMemo(() => records.filter((r) => !isPending(r)), [records]);

  const filteredHistory = useMemo(() => {
    return history.filter((r) => {
      if (historyFilter === "all") return true;
      if (historyFilter === "auto") {
        return r.decision.decision === "approved" && r.decision.by.kind === "policy_auto";
      }
      if (historyFilter === "manual") {
        return r.decision.decision === "approved" && r.decision.by.kind === "human";
      }
      if (historyFilter === "rejected") {
        return r.decision.decision === "rejected";
      }
      return true;
    });
  }, [history, historyFilter]);

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
          {remediationErrorMessage(remsQ.error)}
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
        <div className="space-y-4">
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
        </div>
      </section>

      {/* History / audit timeline */}
      <section className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-line pb-1">
          <h3 className="font-mono text-[11px] uppercase tracking-[0.14em] text-fg-mute">
            History &amp; Audit Logs · {history.length}
          </h3>
          
          {/* Tabs Filter */}
          {history.length > 0 && (
            <div className="flex gap-1 bg-surface-2 p-0.5 rounded-lg border border-line">
              <button
                onClick={() => setHistoryFilter("all")}
                className={`px-2.5 py-1 text-xs font-semibold rounded-md transition-colors ${
                  historyFilter === "all"
                    ? "bg-surface text-fg shadow-sm"
                    : "text-fg-mute hover:text-fg"
                }`}
              >
                All
              </button>
              <button
                onClick={() => setHistoryFilter("auto")}
                className={`px-2.5 py-1 text-xs font-semibold rounded-md transition-colors ${
                  historyFilter === "auto"
                    ? "bg-surface text-fg shadow-sm"
                    : "text-fg-mute hover:text-fg"
                }`}
              >
                Auto-Corrected ({history.filter(r => r.decision.decision === "approved" && r.decision.by.kind === "policy_auto").length})
              </button>
              <button
                onClick={() => setHistoryFilter("manual")}
                className={`px-2.5 py-1 text-xs font-semibold rounded-md transition-colors ${
                  historyFilter === "manual"
                    ? "bg-surface text-fg shadow-sm"
                    : "text-fg-mute hover:text-fg"
                }`}
              >
                Manual ({history.filter(r => r.decision.decision === "approved" && r.decision.by.kind === "human").length})
              </button>
              <button
                onClick={() => setHistoryFilter("rejected")}
                className={`px-2.5 py-1 text-xs font-semibold rounded-md transition-colors ${
                  historyFilter === "rejected"
                    ? "bg-surface text-fg shadow-sm"
                    : "text-fg-mute hover:text-fg"
                }`}
              >
                Rejected ({history.filter(r => r.decision.decision === "rejected").length})
              </button>
            </div>
          )}
        </div>

        {history.length === 0 ? (
          <div className="rounded-xl border border-dashed border-line bg-surface p-10 text-center">
            <p className="text-3xl text-fg-mute">🗄</p>
            <p className="mt-2 font-display text-lg font-bold">No history or audit logs</p>
            <p className="mt-1 text-sm text-fg-mute">
              Completed auto-remediations, manual approvals, and rejections will be recorded here.
            </p>
          </div>
        ) : filteredHistory.length === 0 ? (
          <div className="rounded-xl border border-dashed border-line bg-surface p-8 text-center text-fg-mute text-sm">
            No matching remediation logs found in this filter.
          </div>
        ) : (
          <div className="space-y-4">
            {filteredHistory.map((r) => {
              const p = r.proposal;
              const risk = RISK[p.risk_tier];
              return (
                <div key={p.id} className="rounded-xl border border-line bg-surface p-4 shadow-card">
                  <div className="flex flex-wrap items-center gap-2">
                    <ApprovalBadge record={r} />
                    <span className={`rounded-full border px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider ${risk.cls}`}>
                      {risk.label}
                    </span>
                    <span className="font-semibold">{p.host}</span>
                    <span className="font-mono text-[11px] text-fg-mute">
                      {p.template_id}@{p.template_version}
                    </span>
                    <span className="ml-auto font-mono text-[11px] text-fg-mute" title={absoluteTime(r.updated_at)}>
                      {relativeTime(r.updated_at)}
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

                  <ExecutionStatus record={r} />
                </div>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
