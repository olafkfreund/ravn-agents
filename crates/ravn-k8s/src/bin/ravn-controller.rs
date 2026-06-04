//! ravn-controller (#55): a read-only cluster controller that watches the
//! Kubernetes Events API for workload failure signals — `OOMKilled`,
//! `CrashLoopBackOff`, `BackOff`, `FailedScheduling`, `ImagePullBackOff`,
//! probe `Unhealthy`, evictions — and publishes each as a ravn-core
//! `KubeWorkload` Message to the control plane.
//!
//! It runs as its own binary (not a `ravnd --mode`): the host agent is a small
//! CPU-only daemon, while this needs the heavy kube-rs client and a wholly
//! different in-cluster RBAC/Deployment story (#57/#59). Both share `ravn-core`.

use anyhow::Context;
use kube::Client;
use ravn_k8s::{config::Config, nats::Publisher, watcher};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("RAVN_LOG").unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    tracing::info!(
        agent_id = %config.agent_id.0,
        host = %config.host,
        nats = %config.nats_url,
        "ravn-controller starting"
    );

    let client = Client::try_default()
        .await
        .context("building Kubernetes client (in-cluster or kubeconfig)")?;

    let publisher = Publisher::connect(&config.nats_url, config.agent_id).await?;
    tracing::info!("NATS connected");

    watcher::run(client, config.namespace, config.agent_id, config.host, publisher).await
}
