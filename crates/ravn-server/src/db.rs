//! PostgreSQL access: pool setup, migrations, and message persistence.
//!
//! Uses SQLx's runtime query API (not the `query!` macros) so the build needs
//! no live database — important for the hermetic Nix build.

use anyhow::Context;
use ravn_core::Message;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

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
