//! NATS ingestion: subscribe, validate against the shared schema, persist.

use futures_util::StreamExt;
use ravn_core::Message;

use crate::db;
use crate::state::AppState;

/// Subject the agents publish messages to (`ravn.messages.<agent_id>`).
pub const SUBJECT: &str = "ravn.messages.*";

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
