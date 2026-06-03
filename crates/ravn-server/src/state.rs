//! Shared application state handed to HTTP handlers and the ingestion task.

use sqlx::PgPool;

/// Cloneable handle to the control plane's runtime dependencies.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool.
    pub pool: PgPool,
    /// NATS client (cloneable; shares the underlying connection).
    pub nats: async_nats::Client,
}
