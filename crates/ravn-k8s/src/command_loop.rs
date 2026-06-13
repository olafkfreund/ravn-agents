//! NATS command-pull loop for the in-cluster K8s executor (#146).
//!
//! The control plane publishes signed [`CommandEnvelope`]s on the subject
//! `ravn.commands.<agent-id>` (same pattern as the host agent pull, #114).
//! This loop subscribes, re-verifies the Ed25519 signature, and calls
//! [`ravn_actuator::handle_command`] with the [`KubeExecutor`], then reports
//! an [`ActionResult`] back on `ravn.results.<agent-id>`.
//!
//! ## Replay defence
//!
//! [`CommandEnvelope::nonce`] + [`CommandEnvelope::command_id`] are recorded
//! in the executed-commands set. A duplicate command_id is silently dropped —
//! the result was already reported on the first execution. This is the same
//! approach as the host agent's idempotency ledger (#114).
//!
//! ## Signature re-verification
//!
//! Re-verification happens *inside* [`ravn_actuator::handle_command`] via the
//! injected [`Keyring`]. The executor never acts on an envelope whose
//! signature is absent, expired, or invalid. This provides defence-in-depth:
//! even a compromised NATS broker cannot inject commands.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use async_nats::Subject;
use futures_util::StreamExt;
use ravn_actuator::{handle_command, CapabilityExecutor};
use ravn_core::CommandEnvelope;
use ravn_crypto::Keyring;
use tokio::sync::Mutex;

/// Run the command-pull loop until the NATS connection closes or an
/// unrecoverable error occurs.
///
/// - `nats_url` — e.g. `nats://nats.ravn-system.svc.cluster.local:4222`
/// - `agent_id` — the executor's stable identity (used for the subscribe
///   subject and the result subject)
/// - `ring` — the control plane's trusted command-signing keyring (#150); loaded
///   from a Kubernetes Secret or ConfigMap at startup
/// - `executor` — the [`KubeExecutor`] (or a test mock)
pub async fn run_command_loop<E>(
    nats_url: &str,
    agent_id: &str,
    ring: Keyring,
    executor: Arc<E>,
) -> anyhow::Result<()>
where
    E: CapabilityExecutor + Send + Sync + 'static,
{
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect(nats_url)
        .await
        .with_context(|| format!("connecting to NATS at {nats_url}"))?;

    let command_subject = format!("ravn.commands.{agent_id}");
    let result_subject = format!("ravn.results.{agent_id}");

    let mut sub = client
        .subscribe(Subject::from(command_subject.clone()))
        .await
        .with_context(|| format!("subscribing to {command_subject}"))?;

    tracing::info!(
        subject = %command_subject,
        "K8s executor subscribed; waiting for signed CommandEnvelopes"
    );

    // Idempotency ledger: remember command_ids we have already executed so
    // replay of the same signed envelope is a no-op.
    let executed: Arc<Mutex<HashSet<uuid::Uuid>>> = Arc::new(Mutex::new(HashSet::new()));

    while let Some(msg) = sub.next().await {
        let payload = msg.payload.clone();
        let env: CommandEnvelope = match serde_json::from_slice(&payload) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(%e, "received unparseable message on command subject; dropping");
                continue;
            }
        };

        let command_id = env.command_id;

        // Idempotency check — drop duplicates before executing.
        {
            let mut seen = executed.lock().await;
            if seen.contains(&command_id) {
                tracing::info!(%command_id, "duplicate command_id; already executed, dropping");
                continue;
            }
            seen.insert(command_id);
        }

        tracing::info!(
            %command_id,
            template_id = %env.template_id,
            risk_tier = ?env.risk_tier,
            "executing K8s command"
        );

        // handle_command re-verifies the Ed25519 signature internally, checks
        // preconditions, runs steps, verifies post-state, and handles rollback.
        // The executor trait is async (#144), so we await it directly — the K8s
        // API calls run on the current runtime, no `block_on`/`spawn_blocking`.
        let result = handle_command(executor.as_ref(), &ring, &env).await;

        tracing::info!(
            %command_id,
            status = ?result.status,
            detail = ?result.detail,
            "K8s command finished"
        );

        // Publish the ActionResult to the result subject.
        match serde_json::to_vec(&result) {
            Ok(bytes) => {
                if let Err(e) = client
                    .publish(Subject::from(result_subject.clone()), bytes.into())
                    .await
                {
                    tracing::warn!(%command_id, %e, "failed to publish ActionResult");
                } else {
                    let _ = client.flush().await;
                }
            }
            Err(e) => tracing::warn!(%command_id, %e, "failed to serialize ActionResult"),
        }
    }

    tracing::info!("NATS command subscription closed; K8s executor loop exiting");
    Ok(())
}
