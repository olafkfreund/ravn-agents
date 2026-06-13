//! NATS ingestion: subscribe, validate against the shared schema, persist.

use futures_util::StreamExt;
use ravn_core::{Heartbeat, Message};

use crate::db;
use crate::state::AppState;

/// Subject the agents publish messages to (`ravn.messages.<agent_id>`).
pub const SUBJECT: &str = "ravn.messages.*";
/// Subject the agents publish heartbeats to (`ravn.heartbeat.<agent_id>`).
pub const HEARTBEAT_SUBJECT: &str = "ravn.heartbeat.*";

/// Run the ingestion loop until the subscription ends (e.g. on shutdown).
pub async fn run(state: AppState) {
    let mut sub = match state.nats.subscribe(SUBJECT).await {
        Ok(sub) => sub,
        Err(error) => {
            tracing::error!(%error, subject = SUBJECT, "failed to subscribe; ingestion disabled");
            return;
        }
    };

    // Ensure the SUBSCRIBE reaches the broker before we report ready, so we
    // don't miss messages published immediately after startup (core NATS has
    // no replay).
    let _ = state.nats.flush().await;

    tracing::info!(subject = SUBJECT, "ingestion subscribed");

    while let Some(nats_msg) = sub.next().await {
        match serde_json::from_slice::<Message>(&nats_msg.payload) {
            Ok(message) => persist_message(&state, &message).await,
            Err(error) => {
                // A malformed payload is dropped, not retried — it will never parse.
                state.metrics.ingest_errors.inc();
                tracing::warn!(%error, subject = %nats_msg.subject, "dropping malformed message");
            }
        }
    }

    tracing::info!("ingestion loop ended");
}

/// Persist a validated [`Message`]: insert it, refresh the agent registry,
/// count it, and fan it out to live WebSocket subscribers. Shared by the NATS
/// loop and the authenticated HTTP ingest endpoint (#57).
pub async fn persist_message(state: &AppState, message: &Message) {
    if let Err(error) = db::insert_message(&state.pool, message).await {
        tracing::error!(%error, event_id = %message.event.id, "failed to persist message");
        state.metrics.ingest_errors.inc();
        return;
    }
    if let Err(error) =
        db::touch_agent(&state.pool, message.event.agent_id.0, &message.event.host).await
    {
        tracing::warn!(%error, "failed to update agent registry");
    }
    let severity = serde_json::to_value(message.event.severity)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    state.metrics.events_ingested.with_label_values(&[&severity]).inc();
    // Fan out to live WebSocket subscribers (#29); ignored if none.
    let _ = state.events_tx.send(db::message_to_stored(message));

    // K8s events arrive detection-only; enrich them with an explanation
    // asynchronously, off this path (#58).
    maybe_explain(state, message);

    // Remediation Prepare (#115): best-effort — match a template and record a
    // proposal. Never blocks or fails ingestion.
    crate::remediation::prepare(state, message);
}

/// Spawn an async explanation for a bare K8s event when inference is
/// configured. Off the detection path: a slow or failed call never blocks
/// ingestion, and the event keeps its deterministic title.
fn maybe_explain(state: &AppState, message: &Message) {
    use ravn_core::Source;

    let Some(inference) = state.inference.clone() else { return };
    if message.explanation.is_some() {
        return; // host-agent events already carry their own explanation
    }
    if !matches!(message.event.source(), Source::KubeWorkload | Source::KubeNode) {
        return;
    }

    let event = message.event.clone();
    let pool = state.pool.clone();
    let events_tx = state.events_tx.clone();
    let metrics = state.metrics.clone();

    tokio::spawn(async move {
        let start = std::time::Instant::now();
        match inference.explain(&event).await {
            Ok(explanation) => {
                // #149: record inference latency for successful calls.
                let elapsed = start.elapsed().as_secs_f64();
                metrics.inference_latency_seconds.observe(elapsed);

                if let Err(error) =
                    db::update_explanation(&pool, event.id, event.occurred_at, &explanation).await
                {
                    tracing::warn!(%error, event_id = %event.id, "failed to store explanation");
                    return;
                }
                metrics.explanations_generated.inc();
                // Re-broadcast the now-explained event to live subscribers.
                let enriched = Message { event, explanation: Some(explanation) };
                let _ = events_tx.send(db::message_to_stored(&enriched));
            }
            Err(error) => {
                metrics.explanation_errors.inc();
                // #149: classify timeouts separately so Grafana can distinguish
                // "inference is slow" from "inference is broken".
                let error_str = error.to_string();
                if error_str.contains("timed out") || error_str.contains("deadline") || error_str.contains("timeout") {
                    metrics.inference_timeouts.inc();
                }
                tracing::warn!(%error, event_id = %event.id, "K8s event explanation failed");
            }
        }
    });
}

/// Subscribe to heartbeats and refresh the agent registry (#20).
pub async fn run_heartbeats(state: AppState) {
    let mut sub = match state.nats.subscribe(HEARTBEAT_SUBJECT).await {
        Ok(sub) => sub,
        Err(error) => {
            tracing::error!(%error, subject = HEARTBEAT_SUBJECT, "failed to subscribe to heartbeats");
            return;
        }
    };
    let _ = state.nats.flush().await;
    tracing::info!(subject = HEARTBEAT_SUBJECT, "heartbeat ingestion subscribed");

    while let Some(nats_msg) = sub.next().await {
        match serde_json::from_slice::<Heartbeat>(&nats_msg.payload) {
            Ok(hb) => {
                state.metrics.heartbeats.inc();
                if let Err(error) = db::record_heartbeat(&state.pool, &hb).await {
                    tracing::warn!(%error, "failed to record heartbeat");
                }
            }
            Err(error) => tracing::debug!(%error, "dropping malformed heartbeat"),
        }
    }
}
