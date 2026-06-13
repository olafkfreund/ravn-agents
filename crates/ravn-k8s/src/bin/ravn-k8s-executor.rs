//! ravn-k8s-executor (#146): the in-cluster Kubernetes remediation executor.
//!
//! This binary is the K8s equivalent of `ravn-actuator` on the host. It:
//!
//! 1. Loads the control-plane Ed25519 public key from the environment
//!    (`RAVN_VERIFYING_KEY`, base64-encoded) or a mounted Secret path
//!    (`RAVN_VERIFYING_KEY_FILE`).
//! 2. Subscribes to `ravn.commands.<agent-id>` on NATS.
//! 3. For each received [`ravn_core::CommandEnvelope`]:
//!    a. Re-verifies the Ed25519 signature (defence-in-depth; broker is not
//!       trusted).
//!    b. Validates K8s resource names.
//!    c. Evaluates preconditions against the live cluster.
//!    d. Executes the typed K8s capability steps (`DeletePod`,
//!       `RestartDeployment`).
//!    e. Verifies the post-condition (`PodState`) and performs rollback if
//!       needed.
//! 4. Publishes an [`ravn_core::ActionResult`] to `ravn.results.<agent-id>`.
//!
//! ## Environment variables
//!
//! | Variable                  | Default                             | Notes |
//! |---------------------------|-------------------------------------|-------|
//! | `NATS_URL`                | `nats://127.0.0.1:4222`             | NATS endpoint |
//! | `RAVN_EXECUTOR_ID`        | generated UUID v7                  | Stable K8s executor identity |
//! | `RAVN_CLUSTER`            | `kubernetes`                        | Cluster name (for logs) |
//! | `RAVN_VERIFYING_KEY`      | *required* (or `_FILE`)             | Base64 Ed25519 public key |
//! | `RAVN_VERIFYING_KEY_FILE` | *required* (or direct env var)      | Path to key file |
//!
//! ## RBAC
//!
//! Deploy with the ServiceAccount bound to the Role / RoleBindings in
//! `k8s/rbac/`. The executor needs only:
//! - `pods/delete` in the namespaces it acts on.
//! - `deployments/patch` in the namespaces it acts on.
//! - `pods/list,get` in the namespaces it acts on.

use std::sync::Arc;

use anyhow::Context;
use kube::Client;
use ravn_crypto::{verifying_key_from_b64, VerifyingKey};
use ravn_k8s::{command_loop, executor::KubeExecutor};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RAVN_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // ── Executor identity ────────────────────────────────────────────────────
    let agent_id = match std::env::var("RAVN_EXECUTOR_ID") {
        Ok(s) if !s.is_empty() => {
            Uuid::parse_str(&s).context("RAVN_EXECUTOR_ID must be a UUID")?
        }
        _ => Uuid::now_v7(),
    };

    let cluster = std::env::var("RAVN_CLUSTER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "kubernetes".into());

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());

    // ── Control-plane public key ─────────────────────────────────────────────
    let verifying_key: VerifyingKey = load_verifying_key()?;

    tracing::info!(
        executor_id = %agent_id,
        cluster = %cluster,
        nats = %nats_url,
        "ravn-k8s-executor starting"
    );

    // ── Kubernetes client (in-cluster ServiceAccount) ────────────────────────
    let kube_client = Client::try_default()
        .await
        .context("building Kubernetes client (in-cluster or kubeconfig)")?;

    let executor = Arc::new(KubeExecutor::new(kube_client));

    // ── Command-pull loop ────────────────────────────────────────────────────
    command_loop::run_command_loop(
        &nats_url,
        &agent_id.to_string(),
        verifying_key,
        executor,
    )
    .await
}

/// Load the verifying key from the environment or a mounted file.
///
/// Priority:
/// 1. `RAVN_VERIFYING_KEY` — raw base64 in the environment (acceptable for
///    ConfigMaps; avoid storing private material this way).
/// 2. `RAVN_VERIFYING_KEY_FILE` — path to a file whose single line is the
///    base64 key (preferred: mount the Kubernetes Secret as a volume).
fn load_verifying_key() -> anyhow::Result<VerifyingKey> {
    if let Ok(b64) = std::env::var("RAVN_VERIFYING_KEY") {
        if !b64.is_empty() {
            return verifying_key_from_b64(&b64)
                .map_err(|e| anyhow::anyhow!("RAVN_VERIFYING_KEY: {e}"));
        }
    }

    if let Ok(path) = std::env::var("RAVN_VERIFYING_KEY_FILE") {
        if !path.is_empty() {
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("reading verifying key from {path}"))?;
            return verifying_key_from_b64(contents.trim())
                .map_err(|e| anyhow::anyhow!("RAVN_VERIFYING_KEY_FILE ({path}): {e}"));
        }
    }

    anyhow::bail!(
        "no Ed25519 verifying key provided; set RAVN_VERIFYING_KEY (base64) \
         or RAVN_VERIFYING_KEY_FILE (path to file)"
    )
}
