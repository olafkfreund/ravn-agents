//! Node-level detection for the DaemonSet agent (#56).
//!
//! Watches its own Node object and turns active `.status.conditions` into
//! ravn-core [`KubeNode`](ravn_core::Payload::KubeNode) signals: memory / disk
//! / PID pressure, and `Ready != True` → `NodeNotReady`. Classification reuses
//! `ravn_core::kube_severity_for_reason` so the node-agent and the controller
//! agree on severity.
//!
//! The pure [`node_to_messages`] does the mapping (unit-tested without a
//! cluster); [`run`] drives the informer and emits a signal only when a
//! condition *transitions* into an active state, so a node sitting under
//! sustained pressure yields one signal, not one per watch tick.

use std::collections::HashSet;

use anyhow::Context;
use chrono::Utc;
use futures_util::TryStreamExt;
use k8s_openapi::api::core::v1::Node;
use kube::runtime::watcher::{self, Event as WatchEvent};
use kube::{Api, Client};
use ravn_core::{kube_severity_for_reason, AgentId, Event, KubeNodePayload, Message, Payload};
use uuid::Uuid;

use crate::publish::Publisher;

/// Node conditions that signal trouble when their status is `True`. `Ready` is
/// special-cased (it signals trouble when *not* `True`).
const PRESSURE_CONDITIONS: &[&str] = &["MemoryPressure", "DiskPressure", "PIDPressure"];

/// Map a Node's currently-active conditions to ravn [`Message`]s (one per
/// active condition). Returns empty when the node is healthy.
pub fn node_to_messages(node: &Node, agent_id: AgentId, host: &str) -> Vec<Message> {
    let node_name = node.metadata.name.clone().unwrap_or_else(|| host.to_string());
    let Some(conditions) = node.status.as_ref().and_then(|s| s.conditions.as_ref()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for c in conditions {
        let condition = if c.type_ == "Ready" {
            // Ready False/Unknown ⇒ the node is not ready.
            (c.status != "True").then(|| "NodeNotReady".to_string())
        } else if PRESSURE_CONDITIONS.contains(&c.type_.as_str()) {
            (c.status == "True").then(|| c.type_.clone())
        } else {
            None
        };
        let Some(condition) = condition else { continue };

        let severity = kube_severity_for_reason(&condition);
        let occurred_at =
            c.last_transition_time.as_ref().map(|t| t.0).unwrap_or_else(Utc::now);
        let event = Event {
            id: Uuid::now_v7(),
            occurred_at,
            observed_at: Utc::now(),
            agent_id,
            host: host.to_string(),
            severity,
            title: format!("{condition} on node {node_name}"),
            category_hints: vec![node_name.clone(), "kubernetes".to_string(), "node".to_string()],
            payload: Payload::KubeNode(KubeNodePayload {
                node: node_name.clone(),
                condition,
                message: c.message.clone(),
                extra: Default::default(),
            }),
        };
        out.push(Message::new(event));
    }
    out
}

/// Watch this node's conditions until the stream ends, publishing a signal on
/// each transition into an active (unhealthy) condition.
pub async fn run(
    client: Client,
    node_name: Option<String>,
    agent_id: AgentId,
    host: String,
    publisher: Publisher,
) -> anyhow::Result<()> {
    let api: Api<Node> = Api::all(client);
    let wc = match &node_name {
        Some(n) => watcher::Config::default().fields(&format!("metadata.name={n}")),
        None => watcher::Config::default(),
    };

    // Conditions currently active (key `node/condition`), so we emit only on
    // the healthy→unhealthy edge and re-arm once a condition clears.
    let mut active: HashSet<String> = HashSet::new();

    let stream = watcher::watcher(api, wc);
    futures_util::pin_mut!(stream);
    tracing::info!(?node_name, "watching node conditions");

    while let Some(event) = stream.try_next().await.context("kube node watch stream")? {
        match event {
            // Seed current state from the initial snapshot without emitting.
            WatchEvent::InitApply(node) => {
                reconcile(&mut active, &node, agent_id, &host, false);
            }
            WatchEvent::Apply(node) => {
                for message in reconcile(&mut active, &node, agent_id, &host, true) {
                    match publisher.publish(&message).await {
                        Ok(()) => tracing::info!(
                            severity = ?message.event.severity,
                            title = %message.event.title,
                            "published node signal"
                        ),
                        Err(error) => tracing::warn!(%error, "failed to publish node signal"),
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Update `active` to the node's current condition set and return the messages
/// for conditions that just became active (only when `emit`). Cleared
/// conditions are dropped from `active` so they can fire again later.
fn reconcile(
    active: &mut HashSet<String>,
    node: &Node,
    agent_id: AgentId,
    host: &str,
    emit: bool,
) -> Vec<Message> {
    let node_name = node.metadata.name.clone().unwrap_or_else(|| host.to_string());
    let messages = node_to_messages(node, agent_id, host);

    let current: HashSet<String> =
        messages.iter().map(|m| key(&node_name, condition_of(m))).collect();
    // Drop this node's conditions that are no longer active (re-arm them).
    let prefix = format!("{node_name}/");
    active.retain(|k| !k.starts_with(&prefix) || current.contains(k));

    let mut fresh = Vec::new();
    for message in messages {
        let k = key(&node_name, condition_of(&message));
        if active.insert(k) && emit {
            fresh.push(message);
        }
    }
    fresh
}

fn key(node_name: &str, condition: &str) -> String {
    format!("{node_name}/{condition}")
}

fn condition_of(message: &Message) -> &str {
    match &message.event.payload {
        Payload::KubeNode(p) => &p.condition,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{NodeCondition, NodeStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use ravn_core::Severity;

    fn agent() -> AgentId {
        AgentId(Uuid::nil())
    }

    fn node(name: &str, conditions: Vec<(&str, &str)>) -> Node {
        Node {
            metadata: ObjectMeta { name: Some(name.to_string()), ..Default::default() },
            status: Some(NodeStatus {
                conditions: Some(
                    conditions
                        .into_iter()
                        .map(|(type_, status)| NodeCondition {
                            type_: type_.to_string(),
                            status: status.to_string(),
                            message: Some(format!("{type_} is {status}")),
                            ..Default::default()
                        })
                        .collect(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn healthy_node_yields_no_signals() {
        let n = node("node-a", vec![("Ready", "True"), ("MemoryPressure", "False")]);
        assert!(node_to_messages(&n, agent(), "node-a").is_empty());
    }

    #[test]
    fn memory_pressure_maps_to_kube_node_error() {
        let n = node("node-a", vec![("Ready", "True"), ("MemoryPressure", "True")]);
        let msgs = node_to_messages(&n, agent(), "node-a");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].event.severity, Severity::Error);
        match &msgs[0].event.payload {
            Payload::KubeNode(p) => {
                assert_eq!(p.node, "node-a");
                assert_eq!(p.condition, "MemoryPressure");
            }
            other => panic!("expected kube_node, got {other:?}"),
        }
    }

    #[test]
    fn ready_false_maps_to_not_ready_critical() {
        let n = node("node-a", vec![("Ready", "False")]);
        let msgs = node_to_messages(&n, agent(), "node-a");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].event.severity, Severity::Critical);
        assert_eq!(condition_of(&msgs[0]), "NodeNotReady");
    }

    #[test]
    fn multiple_active_conditions_each_emit() {
        let n = node("node-a", vec![("Ready", "Unknown"), ("DiskPressure", "True"), ("PIDPressure", "True")]);
        let msgs = node_to_messages(&n, agent(), "node-a");
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn reconcile_emits_once_per_transition_and_rearms() {
        let mut active = HashSet::new();
        let pressured = node("node-a", vec![("MemoryPressure", "True")]);
        let healthy = node("node-a", vec![("MemoryPressure", "False")]);

        // First sighting: one fresh signal.
        assert_eq!(reconcile(&mut active, &pressured, agent(), "node-a", true).len(), 1);
        // Still pressured, no transition: no new signal.
        assert_eq!(reconcile(&mut active, &pressured, agent(), "node-a", true).len(), 0);
        // Cleared: no signal, but the condition re-arms.
        assert_eq!(reconcile(&mut active, &healthy, agent(), "node-a", true).len(), 0);
        // Pressured again: a fresh signal fires.
        assert_eq!(reconcile(&mut active, &pressured, agent(), "node-a", true).len(), 1);
    }

    #[test]
    fn seeding_does_not_emit() {
        let mut active = HashSet::new();
        let pressured = node("node-a", vec![("MemoryPressure", "True")]);
        // emit=false seeds state silently...
        assert_eq!(reconcile(&mut active, &pressured, agent(), "node-a", false).len(), 0);
        // ...so a subsequent identical Apply produces nothing (no false alarm
        // for a condition that was already active at startup).
        assert_eq!(reconcile(&mut active, &pressured, agent(), "node-a", true).len(), 0);
    }
}
