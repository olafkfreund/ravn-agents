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

pub use enrollment::{EnrollRequest, EnrollResponse};
pub use event::{AgentId, Event, Severity, Source};
pub use heartbeat::Heartbeat;
pub use message::{Explanation, Message};
pub use payload::{
    AuthPayload, ConfigDriftPayload, Extra, FailedUnitPayload, JournaldPayload, Payload,
    UpdatePayload,
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
}
