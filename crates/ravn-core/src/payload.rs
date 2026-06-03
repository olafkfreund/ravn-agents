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
    KubeWorkload(KubeWorkloadPayload),
    KubeNode(KubeNodePayload),
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
            Payload::KubeWorkload(_) => Source::KubeWorkload,
            Payload::KubeNode(_) => Source::KubeNode,
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

/// A Kubernetes workload signal (#54), sourced by the controller from the
/// Events API and object status: `OOMKilled`, `CrashLoopBackOff`, `BackOff`,
/// `FailedScheduling`, `FailedMount`, `Unhealthy`, `ImagePullBackOff`,
/// evictions, and status transitions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct KubeWorkloadPayload {
    /// Namespace of the involved object.
    pub namespace: String,
    /// Kind of the involved object, e.g. `Pod`, `Deployment`, `Job`. Named
    /// `object_kind` to avoid colliding with the `Payload` enum's `kind` tag.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub object_kind: String,
    /// Name of the involved object.
    pub name: String,
    /// Kubernetes event/condition reason, e.g. `OOMKilled`, `CrashLoopBackOff`.
    pub reason: String,
    /// Human-readable message from the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// How many times the underlying event has fired (K8s `count`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Container the signal concerns, when narrower than the pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A Kubernetes node-level signal (#54), sourced by the DaemonSet node agent:
/// node conditions, kubelet errors, disk/memory pressure, node-level OOM.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct KubeNodePayload {
    /// Node the signal concerns.
    pub node: String,
    /// Node condition or reason, e.g. `MemoryPressure`, `DiskPressure`, `KubeletNotReady`.
    pub condition: String,
    /// Human-readable detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Deterministic severity for a Kubernetes event/condition reason (#54).
///
/// Mirrors the design's mapping so the controller and node agent (#55/#56)
/// classify consistently: hard failures escalate, transient/scheduling issues
/// warn, and status transitions are informational. Unknown reasons default to
/// [`Severity::Warning`] — visible but not alarming.
pub fn kube_severity_for_reason(reason: &str) -> crate::event::Severity {
    use crate::event::Severity::*;
    match reason {
        "OOMKilled" | "Evicted" | "NodeNotReady" | "SystemOOM" => Critical,
        "CrashLoopBackOff" | "BackOff" | "FailedMount" | "FailedCreatePodSandBox" => Error,
        "Unhealthy" | "MemoryPressure" | "DiskPressure" | "PIDPressure" => Error,
        "FailedScheduling" | "ImagePullBackOff" | "ErrImagePull" | "ProbeWarning" => Warning,
        "Created" | "Started" | "Pulled" | "Scheduled" | "SuccessfulCreate" => Info,
        _ => Warning,
    }
}
