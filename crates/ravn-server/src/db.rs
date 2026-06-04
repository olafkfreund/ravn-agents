//! PostgreSQL access: pool setup, migrations, and message persistence.
//!
//! Uses SQLx's runtime query API (not the `query!` macros) so the build needs
//! no live database — important for the hermetic Nix build.

use std::collections::BTreeMap;

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use ravn_core::{Heartbeat, Message};
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

/// Attach (or replace) an event's LLM explanation, generated asynchronously by
/// the control plane for K8s events (#58). The partition key `occurred_at` is
/// in the WHERE so the UPDATE targets a single partition.
pub async fn update_explanation(
    pool: &PgPool,
    id: Uuid,
    occurred_at: DateTime<Utc>,
    explanation: &ravn_core::Explanation,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE events SET explanation = $3 WHERE id = $1 AND occurred_at = $2")
        .bind(id)
        .bind(occurred_at)
        .bind(sqlx::types::Json(explanation))
        .execute(pool)
        .await
        .context("updating event explanation")?;
    Ok(())
}

/// Build the API/wire `StoredEvent` shape from a `Message` (for live fan-out).
pub fn message_to_stored(msg: &Message) -> StoredEvent {
    let ev = &msg.event;
    StoredEvent {
        id: ev.id,
        occurred_at: ev.occurred_at,
        observed_at: ev.observed_at,
        received_at: Utc::now(),
        agent_id: ev.agent_id.0,
        host: ev.host.clone(),
        severity: enum_str(&ev.severity),
        source: enum_str(&ev.source()),
        title: ev.title.clone(),
        category_hints: ev.category_hints.clone(),
        payload: serde_json::to_value(&ev.payload).unwrap_or(serde_json::Value::Null),
        explanation: msg
            .explanation
            .as_ref()
            .map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null)),
    }
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

/// A registered agent with its derived liveness status and labels.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Agent {
    pub agent_id: Uuid,
    pub host: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// `online` | `stale` | `offline`, derived from `last_seen`.
    pub status: String,
    /// User-defined key/value labels.
    pub labels: BTreeMap<String, String>,
    /// Health from the latest heartbeat (#20), if any.
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub uptime_secs: Option<i64>,
    pub inference_enabled: Option<bool>,
    pub events_published: Option<i64>,
}

/// One grouping dimension (label key) and its values.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CategoryDimension {
    pub key: String,
    pub values: Vec<CategoryValue>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CategoryValue {
    pub value: String,
    pub agent_count: i64,
}

#[derive(FromRow)]
struct AgentRow {
    agent_id: Uuid,
    host: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    last_heartbeat: Option<DateTime<Utc>>,
    uptime_secs: Option<i64>,
    inference_enabled: Option<bool>,
    events_published: Option<i64>,
}

const AGENT_COLUMNS: &str =
    "agent_id, host, first_seen, last_seen, last_heartbeat, uptime_secs, inference_enabled, events_published";

#[derive(FromRow)]
struct LabelRow {
    agent_id: Uuid,
    key: String,
    value: String,
}

/// Liveness from last-seen age.
fn agent_status(last_seen: DateTime<Utc>, now: DateTime<Utc>) -> &'static str {
    let age = now - last_seen;
    if age < Duration::seconds(60) {
        "online"
    } else if age < Duration::seconds(300) {
        "stale"
    } else {
        "offline"
    }
}

/// Record that an agent is alive (called on every ingested event).
pub async fn touch_agent(pool: &PgPool, agent_id: Uuid, host: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO agents (agent_id, host, last_seen) VALUES ($1, $2, now())
        ON CONFLICT (agent_id) DO UPDATE SET host = EXCLUDED.host, last_seen = now()
        "#,
    )
    .bind(agent_id)
    .bind(host)
    .execute(pool)
    .await
    .context("touching agent")?;
    Ok(())
}

/// Record a successful enrollment (#19): upsert the agent and stamp its cert
/// issuance/expiry. Idempotent — re-enrolling the same `agent_id` updates in
/// place.
pub async fn record_enrollment(
    pool: &PgPool,
    agent_id: Uuid,
    host: &str,
    cert_not_after: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO agents (agent_id, host, last_seen, enrolled_at, cert_not_after)
        VALUES ($1, $2, now(), now(), $3)
        ON CONFLICT (agent_id) DO UPDATE SET
            host = EXCLUDED.host,
            enrolled_at = now(),
            cert_not_after = EXCLUDED.cert_not_after
        "#,
    )
    .bind(agent_id)
    .bind(host)
    .bind(cert_not_after)
    .execute(pool)
    .await
    .context("recording enrollment")?;
    Ok(())
}

/// All registered agents with status and labels.
pub async fn list_agents(pool: &PgPool) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query_as::<_, AgentRow>(&format!(
        "SELECT {AGENT_COLUMNS} FROM agents ORDER BY host"
    ))
    .fetch_all(pool)
    .await
    .context("listing agents")?;

    let labels = sqlx::query_as::<_, LabelRow>("SELECT agent_id, key, value FROM agent_labels")
        .fetch_all(pool)
        .await
        .context("loading labels")?;

    let mut by_agent: BTreeMap<Uuid, BTreeMap<String, String>> = BTreeMap::new();
    for l in labels {
        by_agent.entry(l.agent_id).or_default().insert(l.key, l.value);
    }

    let now = Utc::now();
    Ok(rows
        .into_iter()
        .map(|r| Agent {
            status: agent_status(r.last_seen, now).to_string(),
            labels: by_agent.remove(&r.agent_id).unwrap_or_default(),
            agent_id: r.agent_id,
            host: r.host,
            first_seen: r.first_seen,
            last_seen: r.last_seen,
            last_heartbeat: r.last_heartbeat,
            uptime_secs: r.uptime_secs,
            inference_enabled: r.inference_enabled,
            events_published: r.events_published,
        })
        .collect())
}

/// A single agent, or `None` if unknown.
pub async fn get_agent(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Agent>> {
    let Some(r) = sqlx::query_as::<_, AgentRow>(&format!(
        "SELECT {AGENT_COLUMNS} FROM agents WHERE agent_id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetching agent")?
    else {
        return Ok(None);
    };

    let labels = sqlx::query_as::<_, LabelRow>(
        "SELECT agent_id, key, value FROM agent_labels WHERE agent_id = $1",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .context("fetching labels")?;

    Ok(Some(Agent {
        status: agent_status(r.last_seen, Utc::now()).to_string(),
        labels: labels.into_iter().map(|l| (l.key, l.value)).collect(),
        agent_id: r.agent_id,
        host: r.host,
        first_seen: r.first_seen,
        last_seen: r.last_seen,
        last_heartbeat: r.last_heartbeat,
        uptime_secs: r.uptime_secs,
        inference_enabled: r.inference_enabled,
        events_published: r.events_published,
    }))
}

/// Record a heartbeat: refresh liveness and store the health snapshot.
pub async fn record_heartbeat(pool: &PgPool, hb: &Heartbeat) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO agents
            (agent_id, host, last_seen, last_heartbeat, uptime_secs, inference_enabled, events_published)
        VALUES ($1, $2, now(), now(), $3, $4, $5)
        ON CONFLICT (agent_id) DO UPDATE SET
            host = EXCLUDED.host,
            last_seen = now(),
            last_heartbeat = now(),
            uptime_secs = EXCLUDED.uptime_secs,
            inference_enabled = EXCLUDED.inference_enabled,
            events_published = EXCLUDED.events_published
        "#,
    )
    .bind(hb.agent_id.0)
    .bind(&hb.host)
    .bind(hb.uptime_secs as i64)
    .bind(hb.inference_enabled)
    .bind(hb.events_published as i64)
    .execute(pool)
    .await
    .context("recording heartbeat")?;
    Ok(())
}

/// Replace an agent's labels. Returns `false` if the agent is unknown.
pub async fn replace_labels(
    pool: &PgPool,
    id: Uuid,
    labels: &BTreeMap<String, String>,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT agent_id FROM agents WHERE agent_id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Ok(false);
    }

    sqlx::query("DELETE FROM agent_labels WHERE agent_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for (key, value) in labels {
        sqlx::query("INSERT INTO agent_labels (agent_id, key, value) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Delete an agent (cascades labels). Returns `false` if unknown.
pub async fn delete_agent(pool: &PgPool, id: Uuid) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM agents WHERE agent_id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting agent")?
        .rows_affected();
    Ok(n > 0)
}

/// Grouping dimensions: each label key with its values and agent counts.
pub async fn list_categories(pool: &PgPool) -> anyhow::Result<Vec<CategoryDimension>> {
    let rows = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT key, value, count(*) FROM agent_labels GROUP BY key, value ORDER BY key, value",
    )
    .fetch_all(pool)
    .await
    .context("listing categories")?;

    let mut dims: Vec<CategoryDimension> = Vec::new();
    for (key, value, count) in rows {
        match dims.last_mut() {
            Some(d) if d.key == key => d.values.push(CategoryValue { value, agent_count: count }),
            _ => dims.push(CategoryDimension {
                key,
                values: vec![CategoryValue { value, agent_count: count }],
            }),
        }
    }
    Ok(dims)
}

/// The fleet shaped for the topology diagram.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Topology {
    /// The label key used for grouping, if any.
    pub group_by: Option<String>,
    /// Available grouping dimensions (label keys).
    pub dimensions: Vec<String>,
    pub groups: Vec<TopologyGroup>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopologyGroup {
    /// Group label value (or "ungrouped"/"all").
    pub key: String,
    pub nodes: Vec<TopologyNode>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopologyNode {
    pub agent_id: Uuid,
    pub host: String,
    pub status: String,
    /// Worst severity among the agent's recent events (last 24h), if any.
    pub severity: Option<String>,
}

fn severity_from_rank(rank: i32) -> &'static str {
    match rank {
        5 => "critical",
        4 => "error",
        3 => "warning",
        2 => "notice",
        _ => "info",
    }
}

/// Build the topology view, grouping agents by the given label key.
pub async fn topology(pool: &PgPool, group_by: Option<&str>) -> anyhow::Result<Topology> {
    let agents = list_agents(pool).await?;

    // Worst recent severity per agent (last 24h).
    let sev_rows = sqlx::query_as::<_, (Uuid, i32)>(
        r#"
        SELECT agent_id, max(CASE severity
            WHEN 'critical' THEN 5 WHEN 'error' THEN 4 WHEN 'warning' THEN 3
            WHEN 'notice' THEN 2 ELSE 1 END)::int
        FROM events
        WHERE occurred_at > now() - interval '24 hours'
        GROUP BY agent_id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("computing worst severity")?;
    let worst: BTreeMap<Uuid, String> = sev_rows
        .into_iter()
        .map(|(id, rank)| (id, severity_from_rank(rank).to_string()))
        .collect();

    let dimensions: Vec<String> = {
        let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for a in &agents {
            keys.extend(a.labels.keys().cloned());
        }
        keys.into_iter().collect()
    };

    let mut grouped: BTreeMap<String, Vec<TopologyNode>> = BTreeMap::new();
    for a in &agents {
        let group_key = match group_by {
            Some(k) => a.labels.get(k).cloned().unwrap_or_else(|| "ungrouped".to_string()),
            None => "all".to_string(),
        };
        grouped.entry(group_key).or_default().push(TopologyNode {
            agent_id: a.agent_id,
            host: a.host.clone(),
            status: a.status.clone(),
            severity: worst.get(&a.agent_id).cloned(),
        });
    }

    Ok(Topology {
        group_by: group_by.map(str::to_string),
        dimensions,
        groups: grouped
            .into_iter()
            .map(|(key, nodes)| TopologyGroup { key, nodes })
            .collect(),
    })
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    #[test]
    fn severity_rank_maps_back() {
        assert_eq!(severity_from_rank(5), "critical");
        assert_eq!(severity_from_rank(3), "warning");
        assert_eq!(severity_from_rank(1), "info");
    }

    #[test]
    fn status_from_last_seen_age() {
        let now = Utc::now();
        assert_eq!(agent_status(now, now), "online");
        assert_eq!(agent_status(now - Duration::seconds(120), now), "stale");
        assert_eq!(agent_status(now - Duration::seconds(600), now), "offline");
    }
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
