//! Source-specific [`Payload`] data, one variant per detection tap (epic #1).
//!
//! Each variant carries the fields known for that tap plus a flattened `extra`
//! map, so a tap can attach additional data without a breaking change to the
//! shared schema.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::Source;

/// Forward-compatibility bag for fields not (yet) modelled explicitly.
pub type Extra = BTreeMap<String, Value>;

/// Structured, source-specific event data, internally tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Journald(JournaldPayload),
    FailedUnit(FailedUnitPayload),
    ConfigDrift(ConfigDriftPayload),
    Auth(AuthPayload),
    Update(UpdatePayload),
}

impl Payload {
    /// The [`Source`] discriminant for this payload.
    pub fn source(&self) -> Source {
        match self {
            Payload::Journald(_) => Source::Journald,
            Payload::FailedUnit(_) => Source::FailedUnit,
            Payload::ConfigDrift(_) => Source::ConfigDrift,
            Payload::Auth(_) => Source::Auth,
            Payload::Update(_) => Source::Update,
        }
    }
}

/// A structured entry from the systemd journal (#9).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct JournaldPayload {
    /// `_SYSTEMD_UNIT`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// syslog priority (0–7), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// `MESSAGE` field.
    pub message: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A systemd unit that entered a failed state, via D-Bus (#10).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct FailedUnitPayload {
    pub unit: String,
    /// systemd result string, e.g. `exit-code`, `timeout`, `oom-kill`.
    pub result: String,
    /// Recent journal lines for context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_log: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A watched config path whose contents changed (#11).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ConfigDriftPayload {
    pub path: String,
    /// Content hash before the change, if a baseline existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_hash: Option<String>,
    /// Content hash after the change.
    pub new_hash: String,
    /// Unified diff, when the file is text and small enough to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// An authentication / SSH / audit event (#12).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct AuthPayload {
    /// What happened, e.g. `ssh_login`, `ssh_failed`, `sudo`, `new_session`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Remote address, when the event has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_addr: Option<String>,
    pub succeeded: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A system update / NixOS-generation change (#13).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePayload {
    /// Update mechanism, e.g. `nixos`, `apt`, `dnf`.
    pub mechanism: String,
    /// Prior version/generation identifier, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// New version/generation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Human-readable list of changes (e.g. closure diff lines).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}
