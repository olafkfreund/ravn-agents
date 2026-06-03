import createClient from "openapi-fetch";
import type { components, paths } from "../api/schema";

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

/** Fetch the topology view, optionally grouped by a label key. */
export async function getTopology(groupBy?: string): Promise<Topology> {
  const { data, error } = await api.GET("/api/topology", {
    params: { query: groupBy ? { group_by: groupBy } : {} },
  });
  if (error || !data) throw new Error("Failed to load topology");
  return data;
}
