import createClient from "openapi-fetch";
import type { components, paths } from "../api/schema";

/** A persisted event, as returned by the control plane. */
export type StoredEvent = components["schemas"]["StoredEvent"];

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
