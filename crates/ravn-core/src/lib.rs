//! Shared types for Ravn — the contract between the agent and the control
//! plane.
//!
//! The spine is the [`Message`] envelope: a deterministic detection [`Event`]
//! (produced by an agent tap, independent of any LLM) plus an optional
//! [`Explanation`] (produced later by local inference). Everything the agent
//! emits and the control plane persists/serves is expressed in these types.

mod enrollment;
mod event;
mod heartbeat;
mod message;
mod payload;
mod remediation;

pub use enrollment::{EnrollRequest, EnrollResponse};
pub use event::{AgentId, Event, Severity, Source};
pub use heartbeat::Heartbeat;
pub use message::{Explanation, Message};
pub use payload::{
    kube_severity_for_reason, AuthPayload, CircuitBreakerTripPayload, ConfigDriftPayload, Extra,
    FailedUnitPayload, JournaldPayload, KubeNodePayload, KubeWorkloadPayload, Payload,
    UpdatePayload,
};
pub use remediation::{
    ActionResult, ActionStatus, ApprovalRef, Capability, CommandEnvelope, Condition, Decision,
    ParamType, ParameterSpec, RemediationProposal, RemediationRecord, RenderError, RiskTier,
    Rollback, Template, TemplateError, TemplateMatch, Verify,
};

/// Crate version, surfaced so the agent and server can report a build identity.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSON Schema for the top-level [`Message`] envelope.
///
/// Regenerate the committed `schema/message.schema.json` with
/// `cargo run --example gen_schema -p ravn-core`.
pub fn message_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_event(payload: Payload) -> Event {
        let now = Utc::now();
        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::now_v7()),
            host: "host-01".into(),
            severity: Severity::Warning,
            title: "something happened".into(),
            category_hints: vec!["prod".into()],
            payload,
        }
    }

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn severity_is_ordered() {
        assert!(Severity::Info < Severity::Critical);
        assert!(Severity::Warning < Severity::Error);
    }

    #[test]
    fn source_is_derived_from_payload() {
        let ev = sample_event(Payload::FailedUnit(FailedUnitPayload {
            unit: "nginx.service".into(),
            result: "exit-code".into(),
            recent_log: vec!["boom".into()],
            ..Default::default()
        }));
        assert_eq!(ev.source(), Source::FailedUnit);
    }

    #[test]
    fn message_round_trips_through_json() {
        let msg = Message::new(sample_event(Payload::Journald(JournaldPayload {
            unit: Some("sshd.service".into()),
            priority: Some(3),
            message: "auth failure".into(),
            ..Default::default()
        })));
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn payload_is_internally_tagged_and_extra_is_captured() {
        // `kind` selects the variant; unknown fields land in `extra`.
        let json = r#"{
            "kind": "config_drift",
            "path": "/etc/nixos/configuration.nix",
            "new_hash": "abc123",
            "generation": 42
        }"#;
        let payload: Payload = serde_json::from_str(json).unwrap();
        match &payload {
            Payload::ConfigDrift(p) => {
                assert_eq!(p.path, "/etc/nixos/configuration.nix");
                assert_eq!(p.extra.get("generation").and_then(|v| v.as_i64()), Some(42));
            }
            other => panic!("expected config_drift, got {other:?}"),
        }
        assert_eq!(payload.source(), Source::ConfigDrift);
    }

    #[test]
    fn message_schema_builds() {
        let schema = message_schema();
        assert!(serde_json::to_value(&schema).unwrap().is_object());
    }

    #[test]
    fn old_schema_message_still_deserializes() {
        // A pre-K8s (#54) message must still parse after the new variants are
        // added — adding tagged enum variants is backward compatible (D7).
        let json = r#"{
            "event": {
                "id": "00000000-0000-7000-8000-000000000001",
                "occurred_at": "2026-01-01T00:00:00Z",
                "observed_at": "2026-01-01T00:00:00Z",
                "agent_id": "00000000-0000-7000-8000-0000000000aa",
                "host": "legacy-01",
                "severity": "warning",
                "title": "sshd restarted",
                "payload": { "kind": "journald", "unit": "sshd.service", "message": "restarted" }
            }
        }"#;
        let msg: Message = serde_json::from_str(json).expect("old-schema message must still parse");
        assert_eq!(msg.event.source(), Source::Journald);
        assert!(msg.explanation.is_none());
    }

    #[test]
    fn kube_workload_payload_round_trips_with_extra() {
        // The `kind` tag selects the variant; unmodelled fields land in `extra`.
        // (`kind` the workload-object field is absent here, so it defaults.)
        let json = r#"{
            "kind": "kube_workload",
            "namespace": "payments",
            "name": "api-7d9",
            "reason": "CrashLoopBackOff",
            "message": "back-off restarting failed container",
            "count": 7,
            "restart_total": 21
        }"#;
        let payload: Payload = serde_json::from_str(json).unwrap();
        match &payload {
            Payload::KubeWorkload(p) => {
                assert_eq!(p.namespace, "payments");
                assert_eq!(p.reason, "CrashLoopBackOff");
                assert_eq!(p.count, Some(7));
                // The object kind is absent here, so it defaults.
                assert!(p.object_kind.is_empty());
                assert_eq!(p.extra.get("restart_total").and_then(|v| v.as_i64()), Some(21));
            }
            other => panic!("expected kube_workload, got {other:?}"),
        }
        assert_eq!(payload.source(), Source::KubeWorkload);
    }

    #[test]
    fn kube_severity_mapping_is_deterministic() {
        assert_eq!(kube_severity_for_reason("OOMKilled"), Severity::Critical);
        assert_eq!(kube_severity_for_reason("CrashLoopBackOff"), Severity::Error);
        assert_eq!(kube_severity_for_reason("FailedScheduling"), Severity::Warning);
        assert_eq!(kube_severity_for_reason("Started"), Severity::Info);
        // Unknown reasons are visible but not alarming.
        assert_eq!(kube_severity_for_reason("SomethingNew"), Severity::Warning);
    }
}
