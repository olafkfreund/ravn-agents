//! The watch loop: a kube-rs informer over core/v1 Events, mapped to ravn
//! [`Message`]s and published to NATS.
//!
//! Events are watched (not polled) so signals surface promptly. The kubelet
//! re-records the same aggregated Event as a workload keeps failing, so we
//! de-duplicate on the Event UID and only re-emit when its `count` advances —
//! one signal per genuine new occurrence, not one per watch tick.

use std::collections::HashMap;

use anyhow::Context;
use futures_util::TryStreamExt;
use k8s_openapi::api::core::v1::Event as KubeEvent;
use kube::runtime::watcher::{self, Event as WatchEvent};
use kube::{Api, Client};
use ravn_core::AgentId;

use crate::mapping::event_to_message;
use crate::nats::Publisher;

/// Watch the cluster for workload signals until the stream ends.
pub async fn run(
    client: Client,
    namespace: Option<String>,
    agent_id: AgentId,
    host: String,
    publisher: Publisher,
) -> anyhow::Result<()> {
    let api: Api<KubeEvent> = match &namespace {
        Some(ns) => Api::namespaced(client, ns),
        None => Api::all(client),
    };

    // uid -> last emitted aggregated count.
    let mut seen: HashMap<String, i32> = HashMap::new();

    let stream = watcher::watcher(api, watcher::Config::default());
    futures_util::pin_mut!(stream);
    tracing::info!(?namespace, "watching Kubernetes events");

    while let Some(event) = stream.try_next().await.context("kube event watch stream")? {
        match event {
            // Seed dedup from the initial list snapshot without emitting the
            // backlog; only changes after init should fire as signals.
            WatchEvent::InitApply(ev) => remember(&mut seen, &ev),
            WatchEvent::Apply(ev) => {
                if !is_new_or_changed(&mut seen, &ev) {
                    continue;
                }
                if let Some(message) = event_to_message(&ev, agent_id, &host) {
                    match publisher.publish(&message).await {
                        Ok(()) => tracing::info!(
                            reason = ev.reason.as_deref().unwrap_or(""),
                            severity = ?message.event.severity,
                            "published kube workload signal"
                        ),
                        Err(error) => tracing::warn!(%error, "failed to publish kube signal"),
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn uid(ev: &KubeEvent) -> Option<String> {
    ev.metadata.uid.clone()
}

fn remember(seen: &mut HashMap<String, i32>, ev: &KubeEvent) {
    if let Some(k) = uid(ev) {
        seen.insert(k, ev.count.unwrap_or(0));
    }
}

/// Record the Event's current count and report whether it is new or advanced.
/// Events without a UID (rare) are always treated as new.
fn is_new_or_changed(seen: &mut HashMap<String, i32>, ev: &KubeEvent) -> bool {
    let Some(k) = uid(ev) else {
        return true;
    };
    let count = ev.count.unwrap_or(0);
    match seen.insert(k, count) {
        Some(prev) => prev != count,
        None => true,
    }
}
