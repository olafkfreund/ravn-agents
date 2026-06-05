import createClient from "openapi-fetch";
import type { components, paths } from "../api/schema";
import { clearToken, getToken } from "./auth";

/** A persisted event, as returned by the control plane. */
export type StoredEvent = components["schemas"]["StoredEvent"];
/** A registered agent with status and labels. */
export type Agent = components["schemas"]["Agent"];
/** A grouping dimension (label key + values). */
export type CategoryDimension = components["schemas"]["CategoryDimension"];
/** The fleet shaped for the topology diagram. */
export type Topology = components["schemas"]["Topology"];
export type TopologyNode = components["schemas"]["TopologyNode"];

/** Typed control-plane client, generated from the server's OpenAPI spec.
 *  baseUrl is empty: in dev, Vite proxies /api to the backend; in prod the
 *  portal is served from the same origin as the API. */
export const api = createClient<paths>({ baseUrl: "" });

// Attach the user's bearer token (#26) to every API call, and drop it on 401
// so the app falls back to the login screen.
api.use({
  onRequest({ request }) {
    const token = getToken();
    if (token) request.headers.set("Authorization", `Bearer ${token}`);
    return request;
  },
  onResponse({ response }) {
    if (response.status === 401) clearToken();
    return response;
  },
});

/** API access role resolved by the control plane for the current user. */
export type Role = "admin" | "viewer";

/** The caller's role from `/api/me` (admin when auth is disabled). */
export async function getMe(): Promise<Role> {
  const token = getToken();
  const res = await fetch("/api/me", {
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
  if (!res.ok) throw new Error("Failed to resolve role");
  const data = (await res.json()) as { role: Role };
  return data.role;
}

/** Fetch recent events, newest first. */
export async function listEvents(limit = 100): Promise<StoredEvent[]> {
  const { data, error } = await api.GET("/api/events", {
    params: { query: { limit } },
  });
  if (error) throw new Error("Failed to load events");
  return data ?? [];
}

/** Fetch all registered agents. */
export async function listAgents(): Promise<Agent[]> {
  const { data, error } = await api.GET("/api/agents", {});
  if (error) throw new Error("Failed to load agents");
  return data ?? [];
}

/** Fetch grouping dimensions (label keys + values). */
export async function listCategories(): Promise<CategoryDimension[]> {
  const { data, error } = await api.GET("/api/categories", {});
  if (error) throw new Error("Failed to load categories");
  return data ?? [];
}

/** Replace an agent's labels (the category model). */
export async function setLabels(agentId: string, labels: Record<string, string>): Promise<void> {
  const { error } = await api.PUT("/api/agents/{id}/labels", {
    params: { path: { id: agentId } },
    body: labels,
  });
  if (error) throw new Error("Failed to save labels");
}

/** Fetch the topology view, optionally grouped by a label key. */
export async function getTopology(groupBy?: string): Promise<Topology> {
  const { data, error } = await api.GET("/api/topology", {
    params: { query: groupBy ? { group_by: groupBy } : {} },
  });
  if (error || !data) throw new Error("Failed to load topology");
  return data;
}

// ── Remediation (#115/#119) ────────────────────────────────────────────────
// These endpoints are not in the generated OpenAPI schema yet (the ravn-core
// types don't derive utoipa schemas), so they use plain fetch with local types.

export type RiskTier = "safe" | "guarded" | "dangerous";
export type ActionStatus =
  | "succeeded"
  | "precondition_failed"
  | "failed"
  | "rejected"
  | "frozen";

/** Who or what approved an action. */
export type ApprovalRef =
  | { kind: "policy_auto" }
  | { kind: "human"; user: string; approved_at: string };

/** The decision on a proposal (tagged by `decision`). */
export type Decision =
  | { decision: "pending" }
  | { decision: "approved"; by: ApprovalRef }
  | { decision: "rejected"; by: string; at: string; reason?: string | null };

export interface RemediationProposal {
  id: string;
  event_id: string;
  agent_id: string;
  host: string;
  template_id: string;
  template_version: number;
  risk_tier: RiskTier;
  params: Record<string, string>;
  rationale: string;
  created_at: string;
}

export interface ActionResult {
  command_id: string;
  status: ActionStatus;
  detail?: string | null;
  observed_state?: string | null;
  finished_at: string;
}

/** The full lifecycle record of a remediation. */
export interface RemediationRecord {
  proposal: RemediationProposal;
  decision: Decision;
  command_id?: string | null;
  signature?: string | null;
  result?: ActionResult | null;
  updated_at: string;
}

/** Distinguishable failure reasons for the remediation API. */
export type RemediationErrorKind = "unreachable" | "unauthorized" | "not_found" | "server";

/** A failure talking to the remediation API, tagged with its cause. */
export class RemediationError extends Error {
  constructor(public readonly kind: RemediationErrorKind, message: string) {
    super(message);
    this.name = "RemediationError";
  }
}

/** A user-facing message for a remediation API failure, by cause. */
export function remediationErrorMessage(error: unknown): string {
  const kind = error instanceof RemediationError ? error.kind : "unreachable";
  switch (kind) {
    case "not_found":
      return "This control plane has no remediation API. Update ravn-server — the /api/remediations endpoints ship with the self-healing loop (#115).";
    case "unauthorized":
      return "Not authorized to view remediations — sign in with an account that has access.";
    case "server":
      return "The control plane returned an error loading remediations.";
    default:
      return "Couldn’t reach the control plane.";
  }
}

/** How long to wait for the control plane before giving up (ms). Bounds a hung
 *  request so the UI shows the actionable error instead of spinning forever. */
const REMEDIATION_TIMEOUT_MS = 8000;

/** Fetch a remediation endpoint with the bearer token, raising a typed
 *  RemediationError that distinguishes network / timeout / 401-403 / 404 / 5xx. */
async function remediationFetch(input: string, init?: RequestInit): Promise<Response> {
  const token = getToken();
  const headers = new Headers(init?.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REMEDIATION_TIMEOUT_MS);
  let res: Response;
  try {
    res = await fetch(input, { ...init, headers, signal: controller.signal });
  } catch {
    // Network failure or our timeout abort — either way, unreachable.
    throw new RemediationError("unreachable", "control plane unreachable or timed out");
  } finally {
    clearTimeout(timer);
  }
  if (res.status === 401 || res.status === 403) {
    if (res.status === 401) clearToken();
    throw new RemediationError("unauthorized", `unauthorized (${res.status})`);
  }
  if (res.status === 404) {
    throw new RemediationError("not_found", "remediation API not found (404)");
  }
  if (!res.ok) {
    throw new RemediationError("server", `server error (${res.status})`);
  }
  return res;
}

/** List remediation records — pending proposals plus decided/executed history. */
export async function listRemediations(): Promise<RemediationRecord[]> {
  const res = await remediationFetch("/api/remediations");
  return (await res.json()) as RemediationRecord[];
}

/** Approve a pending remediation: signs a command and enqueues it for the agent. */
export async function approveRemediation(id: string): Promise<void> {
  await remediationFetch(`/api/remediations/${id}/approve`, { method: "POST" });
}

/** Reject a pending remediation. */
export async function rejectRemediation(id: string): Promise<void> {
  await remediationFetch(`/api/remediations/${id}/reject`, { method: "POST" });
}
