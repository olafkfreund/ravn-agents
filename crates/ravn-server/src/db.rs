//! PostgreSQL access: pool setup, migrations, and message persistence.
//!
//! Uses SQLx's runtime query API (not the `query!` macros) so the build needs
//! no live database — important for the hermetic Nix build.

use anyhow::Context;
use chrono::{DateTime, Utc};
use ravn_core::Message;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

/// A persisted event as returned by the read API.
///
/// Normalized columns are surfaced directly; `payload` and `explanation` are
/// the raw JSON forms of the `ravn-core` types.
#[derive(Debug, Clone, Serialize, ToSchema, FromRow)]
pub struct StoredEvent {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    /// When the control plane ingested it.
    pub received_at: DateTime<Utc>,
    pub agent_id: Uuid,
    pub host: String,
    pub severity: String,
    pub source: String,
    pub title: String,
    pub category_hints: Vec<String>,
    /// Source-specific payload (`ravn_core::Payload`) as JSON.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// LLM explanation (`ravn_core::Explanation`) as JSON, if present.
    #[schema(value_type = Object)]
    pub explanation: Option<serde_json::Value>,
}

/// Connect to PostgreSQL and verify the connection.
pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .context("connecting to PostgreSQL")
}

/// Apply embedded migrations.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("running migrations")
}

/// Lightweight readiness ping.
pub async fn ping(pool: &PgPool) -> bool {
    sqlx::query("SELECT 1").execute(pool).await.is_ok()
}

/// Persist a message. Idempotent: a duplicate `(id, occurred_at)` is ignored.
pub async fn insert_message(pool: &PgPool, msg: &Message) -> anyhow::Result<()> {
    let ev = &msg.event;
    sqlx::query(
        r#"
        INSERT INTO events
            (id, occurred_at, observed_at, agent_id, host, severity, source,
             title, category_hints, payload, explanation)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (id, occurred_at) DO NOTHING
        "#,
    )
    .bind(ev.id)
    .bind(ev.occurred_at)
    .bind(ev.observed_at)
    .bind(ev.agent_id.0)
    .bind(&ev.host)
    .bind(enum_str(&ev.severity))
    .bind(enum_str(&ev.source()))
    .bind(&ev.title)
    .bind(&ev.category_hints)
    .bind(sqlx::types::Json(&ev.payload))
    .bind(msg.explanation.as_ref().map(sqlx::types::Json))
    .execute(pool)
    .await
    .context("inserting event")?;

    Ok(())
}

/// Fetch the most recent events, newest first.
pub async fn recent_events(pool: &PgPool, limit: i64) -> anyhow::Result<Vec<StoredEvent>> {
    let rows = sqlx::query_as::<_, StoredEvent>(
        r#"
        SELECT id, occurred_at, observed_at, received_at, agent_id, host,
               severity, source, title, category_hints, payload, explanation
        FROM events
        ORDER BY occurred_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("querying recent events")?;

    Ok(rows)
}

/// Render a fieldless serde enum (Severity, Source) as its wire string,
/// e.g. `Severity::Warning -> "warning"`.
fn enum_str<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{Severity, Source};

    #[test]
    fn enum_str_renders_wire_strings() {
        assert_eq!(enum_str(&Severity::Warning), "warning");
        assert_eq!(enum_str(&Severity::Critical), "critical");
        assert_eq!(enum_str(&Source::FailedUnit), "failed_unit");
    }
}
