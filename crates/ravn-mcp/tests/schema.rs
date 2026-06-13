//! Schema-conformance tests (#156): every MCP tool must map to an endpoint the
//! control plane actually serves, and the typed parsing we rely on must match
//! the `ravn-core` contract.
//!
//! We validate against the committed OpenAPI document (`portal/openapi.json`),
//! which is generated from the live `ravn-server` routes. This catches the
//! failure mode that sank the old prototypes: a tool quietly calling an endpoint
//! that no longer exists. Two of our endpoints (the remediation approve/reject
//! mutations) are not yet emitted into the OpenAPI document but are real
//! `ravn-server` routes (`/api/remediations/{id}/...`); those are covered by the
//! ravn-core type round-trip below plus the server's own route tests.

use std::path::PathBuf;

use ravn_mcp::tools;
use serde_json::Value;

/// Load and parse the committed OpenAPI document. Returns `None` if it is
/// missing or empty (e.g. a fresh checkout before the portal build), so the
/// suite degrades to a skip rather than a spurious failure.
fn load_openapi() -> Option<Value> {
    let path = openapi_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    Some(serde_json::from_str(&raw).expect("portal/openapi.json must be valid JSON"))
}

/// Resolve `portal/openapi.json` from the workspace root (two levels up from the
/// crate manifest dir).
fn openapi_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("crate is nested under <workspace>/crates/ravn-mcp")
        .join("portal/openapi.json")
}

/// Collect the set of `(METHOD, path)` operations the OpenAPI document declares.
fn declared_operations(doc: &Value) -> Vec<(String, String)> {
    let mut ops = Vec::new();
    if let Some(paths) = doc.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            if let Some(methods) = item.as_object() {
                for method in methods.keys() {
                    ops.push((method.to_uppercase(), path.clone()));
                }
            }
        }
    }
    ops
}

#[test]
fn read_only_tools_map_to_declared_endpoints() {
    let Some(doc) = load_openapi() else {
        eprintln!("portal/openapi.json missing/empty — skipping endpoint conformance");
        return;
    };
    let ops = declared_operations(&doc);
    let has = |method: &str, path: &str| ops.iter().any(|(m, p)| m == method && p == path);

    // Each read-only tool relays exactly one declared GET endpoint.
    assert!(
        has("GET", "/api/agents"),
        "list_agents → GET /api/agents must exist"
    );
    assert!(
        has("GET", "/api/events"),
        "list_recent_events/get_event → GET /api/events must exist"
    );
    assert!(
        has("GET", "/api/topology"),
        "get_topology → GET /api/topology must exist"
    );
}

#[test]
fn agent_schema_has_fields_the_list_agents_tool_surfaces() {
    let Some(doc) = load_openapi() else {
        eprintln!("portal/openapi.json missing/empty — skipping Agent schema check");
        return;
    };
    let required = doc
        .pointer("/components/schemas/Agent/required")
        .and_then(Value::as_array)
        .expect("Agent schema must declare required fields");
    let required: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
    // These are the fields the tool's description promises a caller will see.
    for field in ["agent_id", "host", "status", "labels", "last_seen"] {
        assert!(
            required.contains(&field),
            "Agent schema must require `{field}`"
        );
    }
}

#[test]
fn stored_event_schema_has_fields_the_event_tools_surface() {
    let Some(doc) = load_openapi() else {
        eprintln!("portal/openapi.json missing/empty — skipping StoredEvent schema check");
        return;
    };
    let required = doc
        .pointer("/components/schemas/StoredEvent/required")
        .and_then(Value::as_array)
        .expect("StoredEvent schema must declare required fields");
    let required: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
    for field in [
        "id", "agent_id", "host", "severity", "source", "title", "payload",
    ] {
        assert!(
            required.contains(&field),
            "StoredEvent schema must require `{field}`"
        );
    }
}

#[test]
fn remediation_records_round_trip_through_core_types() {
    // The `list_remediation_proposals` tool parses the control plane's response
    // into `ravn_core::RemediationRecord` to split pending from decided. Prove a
    // realistic API payload parses and that pending detection works — this is the
    // contract the tool depends on.
    let payload = serde_json::json!([
        {
            "proposal": {
                "id": "018f0000-0000-7000-8000-000000000000",
                "event_id": "018f0000-0000-7000-8000-000000000001",
                "agent_id": "018f0000-0000-7000-8000-000000000002",
                "host": "node-1",
                "template_id": "failed-unit-restart",
                "template_version": 3,
                "risk_tier": "safe",
                "params": { "unit": "nginx.service" },
                "rationale": "nginx.service entered a failed state",
                "created_at": "2026-06-13T00:00:00Z"
            },
            "decision": { "decision": "pending" },
            "updated_at": "2026-06-13T00:00:00Z"
        },
        {
            "proposal": {
                "id": "018f0000-0000-7000-8000-000000000010",
                "event_id": "018f0000-0000-7000-8000-000000000011",
                "agent_id": "018f0000-0000-7000-8000-000000000012",
                "host": "node-2",
                "template_id": "failed-unit-restart",
                "template_version": 3,
                "risk_tier": "safe",
                "rationale": "already handled",
                "created_at": "2026-06-13T00:00:00Z"
            },
            "decision": {
                "decision": "rejected",
                "by": "admin",
                "at": "2026-06-13T00:01:00Z"
            },
            "updated_at": "2026-06-13T00:01:00Z"
        }
    ]);

    let records: Vec<ravn_core::RemediationRecord> = serde_json::from_value(payload)
        .expect("API remediation payload must parse into core types");
    assert_eq!(records.len(), 2);
    let pending = records
        .iter()
        .filter(|r| matches!(r.decision, ravn_core::Decision::Pending))
        .count();
    assert_eq!(pending, 1, "exactly one proposal is pending");
}

#[test]
fn tool_input_schemas_are_well_formed_json_schema() {
    // Every advertised tool (in the mutation-enabled superset) must present an
    // object schema with a `properties` map — the shape MCP clients validate
    // arguments against.
    for tool in tools::tool_catalog(true) {
        let schema = &tool.input_schema;
        assert_eq!(
            schema.get("type").and_then(Value::as_str),
            Some("object"),
            "{}: inputSchema.type must be \"object\"",
            tool.name
        );
        assert!(
            schema
                .get("properties")
                .map(Value::is_object)
                .unwrap_or(false),
            "{}: inputSchema must have a properties object",
            tool.name
        );
    }
}

#[test]
fn exactly_five_read_only_and_two_gated_tools() {
    assert_eq!(tools::read_only_tools().len(), 5);
    assert_eq!(tools::mutating_tools().len(), 2);
}
