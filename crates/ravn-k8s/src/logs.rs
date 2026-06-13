//! Log scraper for Kubernetes pods (#54).
//!
//! Periodically polls container stdout logs for running pods, scans them for
//! error signatures (e.g. `ERROR`, `Exception`, `Fatal`, `unreachable`), and
//! publishes matching lines as `KubeWorkload` log signals to the control plane.

use std::collections::HashSet;
use std::time::Duration;

use chrono::Utc;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{LogParams, ListParams},
    Api, Client,
};
use ravn_core::{AgentId, Event, KubeWorkloadPayload, Message, Payload, Severity};
use uuid::Uuid;

use crate::publish::Publisher;

/// Check if a log line indicates a failure.
pub fn matches_error(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("error")
        || lower.contains("exception")
        || lower.contains("fatal")
        || lower.contains("unreachable")
        || lower.contains("connection failed")
        || lower.contains("db connection")
}

/// Run the log scraping loop.
pub async fn run(
    client: Client,
    namespace: Option<String>,
    agent_id: AgentId,
    host: String,
    publisher: std::sync::Arc<Publisher>,
) {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let namespace = namespace.unwrap_or_else(|| "default".to_string());
    let api: Api<Pod> = Api::namespaced(client, &namespace);

    tracing::info!(?namespace, "starting pod log scraper");

    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let pods = match api.list(&ListParams::default()).await {
            Ok(list) => list.items,
            Err(e) => {
                tracing::warn!(%e, "log scraper: failed to list pods");
                continue;
            }
        };

        // If the cache grows too large, clear it to prevent leaks.
        if seen.len() > 5000 {
            seen.clear();
        }

        for pod in pods {
            let Some(pod_name) = pod.metadata.name.clone() else { continue };
            let Some(status) = &pod.status else { continue };
            if status.phase.as_deref() != Some("Running") {
                continue;
            }

            let log_params = LogParams {
                tail_lines: Some(20),
                ..Default::default()
            };

            let logs = match api.logs(&pod_name, &log_params).await {
                Ok(l) => l,
                Err(_) => continue, // e.g. container still starting
            };

            for line in logs.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if matches_error(trimmed) {
                    let key = (pod_name.clone(), trimmed.to_string());
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.insert(key);

                    // Build a KubeWorkload message for the log error.
                    let occurred_at = Utc::now();
                    let payload = KubeWorkloadPayload {
                        namespace: namespace.clone(),
                        object_kind: "Pod".to_string(),
                        name: pod_name.clone(),
                        reason: "LogError".to_string(),
                        message: Some(trimmed.to_string()),
                        count: Some(1),
                        container: None,
                        extra: Default::default(),
                    };

                    let event = Event {
                        id: Uuid::now_v7(),
                        occurred_at,
                        observed_at: Utc::now(),
                        agent_id,
                        host: host.clone(),
                        severity: Severity::Error,
                        title: format!("LogError in Pod {}/{}: {}", namespace, pod_name, trimmed),
                        category_hints: vec![namespace.clone(), "kubernetes".to_string(), "log".to_string()],
                        payload: Payload::KubeWorkload(payload),
                    };

                    let message = Message::new(event);
                    match publisher.publish(&message).await {
                        Ok(()) => tracing::info!(
                            pod = %pod_name,
                            "published log error signal"
                        ),
                        Err(error) => tracing::warn!(%error, "failed to publish log error signal"),
                    }
                }
            }
        }
    }
}
