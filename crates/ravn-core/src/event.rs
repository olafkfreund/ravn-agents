//! The normalized detection [`Event`] and its supporting enums.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::payload::Payload;

/// Stable identity of an agent, assigned at enrollment (epic #3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct AgentId(pub Uuid);

/// Severity of an event, ordered least-to-most urgent.
///
/// The ordering is meaningful (`Info < Critical`) so the control plane can
/// filter/threshold, and it drives portal colour-coding (#29).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Notice,
    Warning,
    Error,
    Critical,
}

/// Which detection tap produced an event (epic #1).
///
/// Not stored directly on [`Event`] — it is derived from the payload via
/// [`Event::source`] so the two can never disagree. The control plane persists
/// it as an indexed column (#24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Journald,
    FailedUnit,
    ConfigDrift,
    Auth,
    Update,
    /// Kubernetes workload signal from the controller (#54).
    KubeWorkload,
    /// Kubernetes node-level signal from the DaemonSet agent (#54).
    KubeNode,
    /// Synthetic event emitted by the control plane itself for self-observability
    /// (#149): circuit-breaker trips, kill-switch activations, etc.
    RavnInternal,
}

/// A normalized detection event.
///
/// Deterministic: produced by a tap, independent of any LLM ("taps fire the
/// alarm; the model only explains"). Inference enrichment is attached
/// downstream via [`crate::Message`], never here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    /// UUIDv7 — time-sortable, so it doubles as a coarse ordering key and suits
    /// Postgres time-series partitioning (#24).
    pub id: Uuid,
    /// When the underlying thing happened, in UTC.
    pub occurred_at: DateTime<Utc>,
    /// When the agent's tap observed it, in UTC. Equals `occurred_at` when the
    /// source carries no distinct event time.
    pub observed_at: DateTime<Utc>,
    /// The agent that emitted this event.
    pub agent_id: AgentId,
    /// Hostname of the machine the agent runs on.
    pub host: String,
    pub severity: Severity,
    /// Short, deterministic human summary (no LLM involvement).
    pub title: String,
    /// Agent-proposed category tags; the server's category model (#25) groups
    /// the fleet on a chosen dimension drawn from these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub category_hints: Vec<String>,
    /// Source-specific structured data; also determines [`Event::source`].
    pub payload: Payload,
}

impl Event {
    /// The detection tap this event came from, derived from its payload.
    pub fn source(&self) -> Source {
        self.payload.source()
    }
}
