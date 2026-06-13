//! The Ravn MCP tool surface.
//!
//! Doctrine (#156): **read-only by default**. The five read tools are always
//! available. The two mutating tools (`approve_remediation`, `reject_remediation`)
//! are only registered when the operator passes `--allow-mutations` *and*
//! supplies an admin-scoped token — and even then they only *enqueue* a signed,
//! human-attributed command for the agent to pull. Nothing here ever executes a
//! remediation, and nothing auto-approves: a mutation tool call maps 1:1 to the
//! same control-plane endpoint a human would click in the portal.

use serde_json::{json, Value};

use crate::client::ControlPlaneClient;
use crate::protocol::{CallToolResult, ToolAnnotations, ToolDefinition};

/// The maximum number of events `list_recent_events` will request. Matches the
/// control plane's own clamp (`MAX_LIMIT` in `ravn-server`).
const MAX_EVENT_LIMIT: u32 = 500;
/// The default event count when the caller omits `limit`.
const DEFAULT_EVENT_LIMIT: u32 = 50;

/// Every tool name this server can expose. Read-only names are always live;
/// the two mutating names are gated.
pub mod names {
    pub const LIST_AGENTS: &str = "list_agents";
    pub const LIST_RECENT_EVENTS: &str = "list_recent_events";
    pub const GET_EVENT: &str = "get_event";
    pub const GET_TOPOLOGY: &str = "get_topology";
    pub const LIST_REMEDIATION_PROPOSALS: &str = "list_remediation_proposals";
    pub const APPROVE_REMEDIATION: &str = "approve_remediation";
    pub const REJECT_REMEDIATION: &str = "reject_remediation";
}

/// A read-only annotation set: observes, never mutates. `open_world_hint` is
/// true because the data comes from an external system (the control plane).
fn read_only(title: &'static str) -> ToolAnnotations {
    ToolAnnotations {
        title,
        read_only_hint: true,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: true,
    }
}

/// A mutating annotation set. `approve` is non-destructive but state-changing;
/// `reject` likewise. Neither is idempotent (a second call on a now-decided
/// proposal is a no-op error, not a repeat).
fn mutating(title: &'static str) -> ToolAnnotations {
    ToolAnnotations {
        title,
        read_only_hint: false,
        destructive_hint: false,
        idempotent_hint: false,
        open_world_hint: true,
    }
}

/// An empty-object input schema (a tool that takes no arguments).
fn no_args_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// The read-only tool definitions — always available.
pub fn read_only_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: names::LIST_AGENTS,
            description: "List all registered Ravn agents with their derived liveness status \
                 (online/stale/offline), host, labels, and last-seen time. Read-only."
                .to_string(),
            input_schema: no_args_schema(),
            annotations: read_only("List agents"),
        },
        ToolDefinition {
            name: names::LIST_RECENT_EVENTS,
            description: format!(
                "List recent detection events across the fleet, newest first. These are \
                 deterministic detections (not LLM output). Optional `limit` (1–{MAX_EVENT_LIMIT}, \
                 default {DEFAULT_EVENT_LIMIT}). Read-only."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_EVENT_LIMIT,
                        "description": "Maximum number of events to return."
                    }
                },
                "additionalProperties": false
            }),
            annotations: read_only("List recent events"),
        },
        ToolDefinition {
            name: names::GET_EVENT,
            description:
                "Fetch a single detection event by its `id` (UUID). Returns the full stored \
                 event including payload and any LLM explanation. Read-only."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The event UUID."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            annotations: read_only("Get event"),
        },
        ToolDefinition {
            name: names::GET_TOPOLOGY,
            description: "Return the fleet shaped for the topology diagram: agents grouped by an \
                 optional label key (`group_by`, e.g. `env`), each node carrying its status \
                 and worst recent severity. Read-only."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "group_by": {
                        "type": "string",
                        "description": "Label key to group agents by (e.g. `env`). Omit for a single group."
                    }
                },
                "additionalProperties": false
            }),
            annotations: read_only("Get topology"),
        },
        ToolDefinition {
            name: names::LIST_REMEDIATION_PROPOSALS,
            description: "List remediation records, optionally filtered to only those awaiting a \
                 decision (`pending_only`, default true). Each proposal is a pre-authored, \
                 deterministic remediation matched to a detection — never LLM-authored. \
                 Read-only: this does NOT approve or reject anything."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pending_only": {
                        "type": "boolean",
                        "description": "When true (default), return only proposals still awaiting a decision."
                    }
                },
                "additionalProperties": false
            }),
            annotations: read_only("List remediation proposals"),
        },
    ]
}

/// The mutating tool definitions — only registered with `--allow-mutations`.
pub fn mutating_tools() -> Vec<ToolDefinition> {
    let id_schema = json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "The remediation proposal UUID." }
        },
        "required": ["id"],
        "additionalProperties": false
    });
    vec![
        ToolDefinition {
            name: names::APPROVE_REMEDIATION,
            description:
                "Approve a PENDING remediation proposal by `id`: the control plane signs and \
                 enqueues the command for the target agent to pull. The action is attributed \
                 to the supplied admin token. This never executes anything directly and never \
                 auto-approves — it mirrors clicking Approve in the portal. Gated: requires \
                 the server to run with --allow-mutations and an admin-scoped token."
                    .to_string(),
            input_schema: id_schema.clone(),
            annotations: mutating("Approve remediation"),
        },
        ToolDefinition {
            name: names::REJECT_REMEDIATION,
            description:
                "Reject a pending remediation proposal by `id`. Records the rejection in the \
                 audit trail; no command is issued. Gated: requires --allow-mutations and an \
                 admin-scoped token."
                    .to_string(),
            input_schema: id_schema,
            annotations: mutating("Reject remediation"),
        },
    ]
}

/// The set of tools to advertise given whether mutations are enabled.
pub fn tool_catalog(allow_mutations: bool) -> Vec<ToolDefinition> {
    let mut tools = read_only_tools();
    if allow_mutations {
        tools.extend(mutating_tools());
    }
    tools
}

/// Dispatch a `tools/call` to the right handler. `allow_mutations` is enforced
/// here as defence in depth: even if a client somehow names a gated tool that
/// was not advertised, an unmutating server refuses it.
pub async fn dispatch(
    client: &ControlPlaneClient,
    allow_mutations: bool,
    name: &str,
    arguments: &Value,
) -> CallToolResult {
    match name {
        names::LIST_AGENTS => relay(client.list_agents().await),
        names::LIST_RECENT_EVENTS => list_recent_events(client, arguments).await,
        names::GET_EVENT => get_event(client, arguments).await,
        names::GET_TOPOLOGY => get_topology(client, arguments).await,
        names::LIST_REMEDIATION_PROPOSALS => list_remediation_proposals(client, arguments).await,
        names::APPROVE_REMEDIATION | names::REJECT_REMEDIATION => {
            if !allow_mutations {
                return CallToolResult::error(format!(
                    "tool `{name}` is disabled: this MCP server runs read-only. \
                     Restart it with --allow-mutations and an admin token to enable mutations."
                ));
            }
            mutate(client, name, arguments).await
        }
        other => CallToolResult::error(format!("unknown tool: {other}")),
    }
}

/// Relay a client result straight through as a JSON tool result.
fn relay(result: anyhow::Result<Value>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::json(&value),
        Err(error) => CallToolResult::error(format!("{error:#}")),
    }
}

async fn list_recent_events(client: &ControlPlaneClient, args: &Value) -> CallToolResult {
    let limit = match args.get("limit") {
        None | Some(Value::Null) => DEFAULT_EVENT_LIMIT,
        Some(v) => match v.as_u64() {
            Some(n) => (n as u32).clamp(1, MAX_EVENT_LIMIT),
            None => return CallToolResult::error("`limit` must be an integer"),
        },
    };
    relay(client.list_events(limit).await)
}

async fn get_event(client: &ControlPlaneClient, args: &Value) -> CallToolResult {
    let Some(id) = args.get("id").and_then(Value::as_str) else {
        return CallToolResult::error("`id` (string UUID) is required");
    };
    // The control plane has no single-event endpoint; fetch the recent window
    // and select. We request the max so a moderately old event is still found.
    let events = match client.list_events(MAX_EVENT_LIMIT).await {
        Ok(v) => v,
        Err(error) => return CallToolResult::error(format!("{error:#}")),
    };
    let found = events
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(id));
    match found {
        Some(event) => CallToolResult::json(event),
        None => CallToolResult::error(format!(
            "no event with id {id} in the most recent {MAX_EVENT_LIMIT} events"
        )),
    }
}

async fn get_topology(client: &ControlPlaneClient, args: &Value) -> CallToolResult {
    let group_by = args.get("group_by").and_then(Value::as_str);
    relay(client.topology(group_by).await)
}

async fn list_remediation_proposals(client: &ControlPlaneClient, args: &Value) -> CallToolResult {
    // Default to pending-only: the read tool's job is to surface what needs a
    // human decision, not the full history.
    let pending_only = args
        .get("pending_only")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let raw = match client.list_remediations().await {
        Ok(v) => v,
        Err(error) => return CallToolResult::error(format!("{error:#}")),
    };

    if !pending_only {
        return CallToolResult::json(&raw);
    }

    // Parse into typed records so we can reliably tell "pending" from decided,
    // regardless of the JSON shape of the embedded decision tag.
    let records: Vec<ravn_core::RemediationRecord> = match serde_json::from_value(raw.clone()) {
        Ok(r) => r,
        // If the shape ever drifts from our typed view, fall back to relaying
        // the raw list rather than failing the tool.
        Err(_) => return CallToolResult::json(&raw),
    };
    let pending: Vec<&ravn_core::RemediationRecord> = records
        .iter()
        .filter(|r| matches!(r.decision, ravn_core::Decision::Pending))
        .collect();
    match serde_json::to_value(&pending) {
        Ok(v) => CallToolResult::json(&v),
        Err(error) => CallToolResult::error(format!("serializing pending proposals: {error}")),
    }
}

async fn mutate(client: &ControlPlaneClient, name: &str, args: &Value) -> CallToolResult {
    let Some(id) = args.get("id").and_then(Value::as_str) else {
        return CallToolResult::error("`id` (string UUID) is required");
    };
    let result = match name {
        names::APPROVE_REMEDIATION => client.approve_remediation(id).await,
        names::REJECT_REMEDIATION => client.reject_remediation(id).await,
        _ => unreachable!("mutate called with non-mutating tool {name}"),
    };
    relay(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_catalog_excludes_mutations() {
        let tools = tool_catalog(false);
        let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        assert!(names.contains(&names::LIST_AGENTS));
        assert!(names.contains(&names::LIST_RECENT_EVENTS));
        assert!(names.contains(&names::GET_EVENT));
        assert!(names.contains(&names::GET_TOPOLOGY));
        assert!(names.contains(&names::LIST_REMEDIATION_PROPOSALS));
        assert!(!names.contains(&names::APPROVE_REMEDIATION));
        assert!(!names.contains(&names::REJECT_REMEDIATION));
        assert_eq!(tools.len(), 5);
    }

    #[test]
    fn mutating_catalog_adds_two_gated_tools() {
        let tools = tool_catalog(true);
        let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        assert!(names.contains(&names::APPROVE_REMEDIATION));
        assert!(names.contains(&names::REJECT_REMEDIATION));
        assert_eq!(tools.len(), 7);
    }

    #[test]
    fn read_only_tools_are_annotated_read_only() {
        for tool in read_only_tools() {
            assert!(
                tool.annotations.read_only_hint,
                "{} must be read-only",
                tool.name
            );
        }
    }

    #[test]
    fn mutating_tools_are_annotated_not_read_only() {
        for tool in mutating_tools() {
            assert!(
                !tool.annotations.read_only_hint,
                "{} must not be read-only",
                tool.name
            );
        }
    }

    #[test]
    fn every_tool_has_an_object_input_schema() {
        for tool in tool_catalog(true) {
            assert_eq!(
                tool.input_schema.get("type").and_then(Value::as_str),
                Some("object"),
                "{} inputSchema must be a JSON Schema object",
                tool.name
            );
        }
    }
}
