//! ravn-node-agent (#56): a DaemonSet (one pod per node) that watches its own
//! Node's `.status.conditions` and publishes node-level problems — memory /
//! disk / PID pressure, `NodeNotReady` — as ravn-core `KubeNode` Messages.
//!
//! A separate binary from the controller (#55) because it has a different
//! deployment shape (DaemonSet + downward-API `NODE_NAME` + tightly-scoped
//! securityContext, #59) and a node-scoped watch, but it shares this crate's
//! config + NATS publisher and `ravn-core` types.
//!
//! Container-stdout and node-OS journald (via read-only hostPath) taps listed
//! in #56 are deferred to the manifest work (#59), where the hostPath/security
//! context wiring lives.

use anyhow::Context;
use kube::Client;
use ravn_k8s::{config::Config, publish::Publisher, node};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("RAVN_LOG").unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    // On a node, record the node name as the event host; fall back to the
    // generic host identity when run outside a DaemonSet.
    let host = config.node_name.clone().unwrap_or_else(|| config.host.clone());
    tracing::info!(
        agent_id = %config.agent_id.0,
        node = %host,
        nats = %config.nats_url,
        "ravn-node-agent starting"
    );

    let client = Client::try_default()
        .await
        .context("building Kubernetes client (in-cluster or kubeconfig)")?;

    let publisher = Publisher::connect(&config).await?;

    node::run(client, config.node_name, config.agent_id, host, publisher).await
}
