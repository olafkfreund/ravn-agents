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
            Ok(message) => {
                if let Err(error) = db::insert_message(&state.pool, &message).await {
                    tracing::error!(%error, event_id = %message.event.id, "failed to persist message");
                } else {
                    if let Err(error) =
                        db::touch_agent(&state.pool, message.event.agent_id.0, &message.event.host).await
                    {
                        tracing::warn!(%error, "failed to update agent registry");
                    }
                    // Fan out to live WebSocket subscribers (#29); ignored if none.
                    let _ = state.events_tx.send(db::message_to_stored(&message));
                }
            }
            Err(error) => {
                // A malformed payload is dropped, not retried — it will never
                // parse. Counted via metrics later (#40).
                tracing::warn!(%error, subject = %nats_msg.subject, "dropping malformed message");
            }
        }
    }

    tracing::info!("ingestion loop ended");
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
                if let Err(error) = db::record_heartbeat(&state.pool, &hb).await {
                    tracing::warn!(%error, "failed to record heartbeat");
                }
            }
            Err(error) => tracing::debug!(%error, "dropping malformed heartbeat"),
        }
    }
}
