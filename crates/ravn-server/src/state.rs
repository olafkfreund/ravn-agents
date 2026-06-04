//! Shared application state handed to HTTP handlers and the ingestion task.

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::auth::IngestAuth;
use crate::ca::Ca;
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
    /// Internal CA for agent enrollment (#19). `None` disables `/enroll`.
    pub ca: Option<Arc<Ca>>,
    /// Bootstrap token agents present to enroll. Paired with `ca`.
    pub enroll_token: Option<String>,
    /// ServiceAccount-token validator for the authenticated HTTP ingest
    /// endpoint (#57). `None` disables `/ingest`.
    pub ingest_auth: Option<Arc<IngestAuth>>,
}
