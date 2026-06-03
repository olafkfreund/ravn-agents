//! Shared application state handed to HTTP handlers and the ingestion task.

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::db::StoredEvent;
use crate::metrics::Metrics;

/// Cloneable handle to the control plane's runtime dependencies.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool.
    pub pool: PgPool,
    /// NATS client (cloneable; shares the underlying connection).
    pub nats: async_nats::Client,
    /// Fan-out of freshly ingested events to live WebSocket subscribers (#29).
    pub events_tx: broadcast::Sender<StoredEvent>,
    /// API auth tokens (#26). Auth is enforced only when at least one is set.
    pub admin_token: Option<String>,
    pub viewer_token: Option<String>,
    /// Prometheus metrics (#40).
    pub metrics: Arc<Metrics>,
}
