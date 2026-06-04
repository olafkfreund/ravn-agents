//! Pure mapping from a Kubernetes core/v1 [`Event`] to a ravn-core [`Message`].
//!
//! Kept free of any I/O so the classification logic is unit-testable without a
//! cluster: the controller's watcher (`watcher.rs`) just feeds objects through
//! here. Severity comes from the shared [`kube_severity_for_reason`] table (#54)
//! so the controller and the node agent (#56) stay consistent.

use chrono::Utc;
use k8s_openapi::api::core::v1::Event as KubeEvent;
use ravn_core::{
    kube_severity_for_reason, AgentId, Event, KubeWorkloadPayload, Message, Payload, Severity,
};
use uuid::Uuid;

/// Turn a Kubernetes Event into a ravn [`Message`], or `None` if it is routine
/// noise not worth surfacing.
///
/// We keep `Warning`-type events and anything the reason table rates `Error` or
/// worse; ordinary `Normal` lifecycle events (`Started`, `Pulled`, …) are
/// dropped so the fleet view isn't flooded.
pub fn event_to_message(ev: &KubeEvent, agent_id: AgentId, host: &str) -> Option<Message> {
    let reason = ev.reason.clone()?;
    let severity = kube_severity_for_reason(&reason);
    let is_warning = ev.type_.as_deref() == Some("Warning");
    if !is_warning && severity < Severity::Error {
        return None;
    }

    let obj = &ev.involved_object;
    let namespace = obj.namespace.clone().unwrap_or_default();
    let object_kind = obj.kind.clone().unwrap_or_default();
    let name = obj.name.clone().unwrap_or_default();
    let container = obj.field_path.as_deref().and_then(container_from_field_path);
    let count = ev.count.map(|c| c.max(0) as u32);

    let occurred_at = ev
        .last_timestamp
        .as_ref()
        .or(ev.first_timestamp.as_ref())
        .map(|t| t.0)
        .unwrap_or_else(Utc::now);

    let title = if object_kind.is_empty() {
        format!("{reason} in {namespace}")
    } else {
        format!("{reason}: {object_kind} {namespace}/{name}")
    };

    let payload = KubeWorkloadPayload {
        namespace: namespace.clone(),
        object_kind,
        name,
        reason,
        message: ev.message.clone(),
        count,
        container,
        extra: Default::default(),
    };

    let event = Event {
        id: Uuid::now_v7(),
        occurred_at,
        observed_at: Utc::now(),
        agent_id,
        host: host.to_string(),
        severity,
        title,
        category_hints: vec![namespace, "kubernetes".to_string()],
        payload: Payload::KubeWorkload(payload),
    };
    Some(Message::new(event))
}

/// Extract the container name from an `involvedObject.fieldPath` such as
/// `spec.containers{crasher}` → `crasher`.
fn container_from_field_path(field_path: &str) -> Option<String> {
    let start = field_path.find('{')? + 1;
    let end = field_path[start..].find('}')? + start;
    let name = &field_path[start..end];
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::ObjectReference;

    fn agent() -> AgentId {
        AgentId(Uuid::nil())
    }

    fn kube_event(reason: &str, type_: &str) -> KubeEvent {
        KubeEvent {
            reason: Some(reason.to_string()),
            type_: Some(type_.to_string()),
            message: Some("back-off restarting failed container".to_string()),
            count: Some(7),
            involved_object: ObjectReference {
                namespace: Some("ravn-test".to_string()),
                kind: Some("Pod".to_string()),
                name: Some("crasher".to_string()),
                field_path: Some("spec.containers{crasher}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn warning_backoff_maps_to_kube_workload() {
        let msg = event_to_message(&kube_event("BackOff", "Warning"), agent(), "ravn-dev")
            .expect("a Warning BackOff must surface");
        assert_eq!(msg.event.severity, Severity::Error);
        assert_eq!(msg.event.host, "ravn-dev");
        match &msg.event.payload {
            Payload::KubeWorkload(p) => {
                assert_eq!(p.namespace, "ravn-test");
                assert_eq!(p.object_kind, "Pod");
                assert_eq!(p.name, "crasher");
                assert_eq!(p.reason, "BackOff");
                assert_eq!(p.count, Some(7));
                assert_eq!(p.container.as_deref(), Some("crasher"));
            }
            other => panic!("expected kube_workload, got {other:?}"),
        }
    }

    #[test]
    fn normal_lifecycle_events_are_dropped() {
        // A routine `Normal`/`Started` event is noise and must not surface.
        assert!(event_to_message(&kube_event("Started", "Normal"), agent(), "h").is_none());
        assert!(event_to_message(&kube_event("Pulled", "Normal"), agent(), "h").is_none());
    }

    #[test]
    fn normal_but_severe_reason_still_surfaces() {
        // OOMKilled rates Critical, so it surfaces even if reported as Normal.
        let msg = event_to_message(&kube_event("OOMKilled", "Normal"), agent(), "h")
            .expect("a severe reason must surface regardless of type");
        assert_eq!(msg.event.severity, Severity::Critical);
    }

    #[test]
    fn event_without_reason_is_skipped() {
        let mut ev = kube_event("BackOff", "Warning");
        ev.reason = None;
        assert!(event_to_message(&ev, agent(), "h").is_none());
    }

    #[test]
    fn field_path_without_container_yields_none() {
        assert_eq!(container_from_field_path("spec.containers{web}").as_deref(), Some("web"));
        assert_eq!(container_from_field_path("status"), None);
        assert_eq!(container_from_field_path("spec.containers{}"), None);
    }
}
