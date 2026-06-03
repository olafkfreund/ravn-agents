//! Agent [`Heartbeat`] — periodic health, separate from detection events (#20).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::event::AgentId;

/// A periodic liveness + health report from an agent.
///
/// Published on its own transport subject (`ravn.heartbeat.<agent_id>`), it
/// lets the control plane mark agents online/stale/offline even when they are
/// quiet (emitting no events).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Heartbeat {
    pub agent_id: AgentId,
    pub host: String,
    /// When this heartbeat was sent (UTC).
    pub sent_at: DateTime<Utc>,
    /// Agent process uptime, in seconds.
    pub uptime_secs: u64,
    /// When the agent last emitted a detection event, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_detection: Option<DateTime<Utc>>,
    /// Whether local inference is enabled.
    pub inference_enabled: bool,
    /// Total events published since start.
    pub events_published: u64,
}
