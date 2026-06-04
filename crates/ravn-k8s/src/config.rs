//! Runtime configuration for the controller, all from the environment so it
//! drops cleanly into a Deployment manifest (#59).

use anyhow::Context;
use ravn_core::AgentId;
use uuid::Uuid;

/// Controller configuration.
pub struct Config {
    /// NATS URL of the control plane (`NATS_URL`). Default matches the agent.
    pub nats_url: String,
    /// Stable identity used as the publish subject and the event `agent_id`.
    /// `RAVN_CONTROLLER_ID` (a UUID); generated if unset.
    pub agent_id: AgentId,
    /// Cluster identity recorded as the event `host` (`RAVN_CLUSTER`).
    pub host: String,
    /// Restrict the watch to one namespace (`RAVN_K8S_NAMESPACE`); `None` =
    /// all namespaces (the production default — controller has cluster-wide
    /// read-only RBAC).
    pub namespace: Option<String>,
    /// The node this agent runs on (`NODE_NAME`, set via the downward API in
    /// the DaemonSet). `None` outside a node context; the node-agent restricts
    /// its watch to this node and records it as the event `host`.
    pub node_name: Option<String>,
    /// Authenticated HTTP ingest URL (`RAVN_INGEST_URL`, e.g.
    /// `https://ravn-control-plane/ingest`). When set, the agent publishes over
    /// HTTP with its ServiceAccount token (#57) instead of to NATS.
    pub ingest_url: Option<String>,
    /// Path to the projected ServiceAccount token presented to the ingest
    /// endpoint (`RAVN_SA_TOKEN_FILE`). Re-read per publish so token rotation
    /// is picked up.
    pub sa_token_file: String,
}

impl Config {
    /// Build configuration from environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());

        let agent_id = match std::env::var("RAVN_CONTROLLER_ID") {
            Ok(s) if !s.is_empty() => {
                AgentId(Uuid::parse_str(&s).context("RAVN_CONTROLLER_ID must be a UUID")?)
            }
            _ => AgentId(Uuid::now_v7()),
        };

        let host = std::env::var("RAVN_CLUSTER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "kubernetes".into());

        let namespace = std::env::var("RAVN_K8S_NAMESPACE").ok().filter(|s| !s.is_empty());

        let node_name = std::env::var("NODE_NAME").ok().filter(|s| !s.is_empty());

        let ingest_url = std::env::var("RAVN_INGEST_URL").ok().filter(|s| !s.is_empty());
        let sa_token_file = std::env::var("RAVN_SA_TOKEN_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/var/run/secrets/ravn/token".to_string());

        Ok(Self { nats_url, agent_id, host, namespace, node_name, ingest_url, sa_token_file })
    }
}
