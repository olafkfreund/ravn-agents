//! Remediation command signing and the per-agent command channel (#114).
//!
//! The control plane holds an Ed25519 signing key. The orchestrator (#115)
//! resolves a [`CommandEnvelope`], signs it with [`CommandSigner`], and enqueues
//! it; the agent pulls it from [`CommandQueue`] over its outbound connection,
//! verifies the signature against the pinned public key (delivered at enrollment),
//! executes via the actuator, and POSTs an [`ActionResult`] back.
//!
//! The queue is in-memory for P1: a pending command lives only until the agent
//! pulls it. Durable audit of the whole lifecycle is the orchestrator's job
//! (#115, Postgres `RemediationRecord`).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use anyhow::Context;
use ravn_core::{ActionResult, CommandEnvelope};
use ravn_crypto::{
    generate_signing_key, signing_key_from_b64, signing_key_to_b64, verifying_key_to_b64,
    SigningKey,
};
use uuid::Uuid;

/// Holds the control plane's Ed25519 signing key and the base64 public key it
/// advertises to agents at enrollment.
pub struct CommandSigner {
    // Read by `sign`, which the orchestrator (#115) calls. The public key is
    // already in use today (advertised at enrollment).
    #[allow(dead_code)]
    key: SigningKey,
    pubkey_b64: String,
}

impl CommandSigner {
    /// Load the signing key from `path` (base64), generating and persisting one
    /// (mode `0600`) if the file is absent. With `path = None`, generate an
    /// ephemeral key that does not survive a restart (logged as a warning) —
    /// acceptable for dev, never for a real fleet.
    pub fn load_or_generate(path: Option<&str>) -> anyhow::Result<Self> {
        let key = match path {
            Some(p) if std::path::Path::new(p).exists() => {
                let b64 = std::fs::read_to_string(p)
                    .with_context(|| format!("reading command signing key at {p}"))?;
                signing_key_from_b64(&b64)
                    .map_err(|e| anyhow::anyhow!("invalid command signing key at {p}: {e}"))?
            }
            Some(p) => {
                let key = generate_signing_key();
                persist_key(p, &key).with_context(|| format!("persisting command signing key to {p}"))?;
                tracing::info!(path = %p, "generated new command signing key");
                key
            }
            None => {
                tracing::warn!(
                    "no RAVN_COMMAND_KEY set — using an ephemeral command signing key that will \
                     not survive a restart; agents must re-enroll to re-pin. Set RAVN_COMMAND_KEY \
                     for a real deployment."
                );
                generate_signing_key()
            }
        };
        let pubkey_b64 = verifying_key_to_b64(&key.verifying_key());
        Ok(Self { key, pubkey_b64 })
    }

    /// Sign an envelope in place. Called by the orchestrator (#115) when a
    /// remediation is approved/auto-approved.
    #[allow(dead_code)] // wired in by the orchestrator (#115)
    pub fn sign(&self, env: &mut CommandEnvelope) {
        ravn_crypto::sign_envelope(&self.key, env);
    }

    /// The base64 public key advertised to agents at enrollment.
    pub fn pubkey_b64(&self) -> &str {
        &self.pubkey_b64
    }
}

/// Write a signing key as base64 with owner-only (`0600`) permissions.
fn persist_key(path: &str, key: &SigningKey) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(signing_key_to_b64(key).as_bytes())?;
    Ok(())
}

/// In-memory per-agent queue of signed commands awaiting pull, plus the reported
/// results. Cloneable handles share one backing store via `Arc` in `AppState`.
#[derive(Default)]
pub struct CommandQueue {
    pending: Mutex<HashMap<Uuid, VecDeque<CommandEnvelope>>>,
    results: Mutex<Vec<ActionResult>>,
}

impl CommandQueue {
    /// Enqueue a signed command for its target agent (called by the orchestrator).
    #[allow(dead_code)] // wired in by the orchestrator (#115)
    pub fn enqueue(&self, env: CommandEnvelope) {
        self.pending
            .lock()
            .expect("command queue mutex poisoned")
            .entry(env.agent_id.0)
            .or_default()
            .push_back(env);
    }

    /// Take (and remove) all pending commands for an agent — the pull endpoint.
    pub fn take_for(&self, agent_id: Uuid) -> Vec<CommandEnvelope> {
        let mut pending = self.pending.lock().expect("command queue mutex poisoned");
        match pending.get_mut(&agent_id) {
            Some(q) => q.drain(..).collect(),
            None => Vec::new(),
        }
    }

    /// Record a result reported by an agent.
    pub fn record_result(&self, result: ActionResult) {
        self.results.lock().expect("results mutex poisoned").push(result);
    }

    /// All recorded results (audit/inspection; superseded by #115's DB store).
    #[allow(dead_code)] // inspection seam; #115 persists results to Postgres
    pub fn results(&self) -> Vec<ActionResult> {
        self.results.lock().expect("results mutex poisoned").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{ActionStatus, AgentId, ApprovalRef, Capability, RiskTier};
    use ravn_crypto::{verify_envelope, verifying_key_from_b64};

    fn envelope(agent: Uuid) -> CommandEnvelope {
        let now = chrono::Utc::now();
        CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(agent),
            template_id: "failed-unit-restart".into(),
            template_version: 3,
            risk_tier: RiskTier::Safe,
            steps: vec![Capability::RestartUnit { unit: "nginx.service".into() }],
            verify: None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "n".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            sig: None,
        }
    }

    #[test]
    fn ephemeral_signer_signs_verifiably() {
        let signer = CommandSigner::load_or_generate(None).unwrap();
        let mut env = envelope(Uuid::now_v7());
        signer.sign(&mut env);
        let pubkey = verifying_key_from_b64(signer.pubkey_b64()).unwrap();
        verify_envelope(&pubkey, &env, chrono::Utc::now()).unwrap();
    }

    #[test]
    fn signer_persists_and_reloads_the_same_key() {
        let dir = std::env::temp_dir().join(format!("ravn-cmdkey-{}", std::process::id()));
        let path = dir.join("command.key");
        let path_str = path.to_str().unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        let first = CommandSigner::load_or_generate(Some(path_str)).unwrap();
        let pubkey_b64 = first.pubkey_b64().to_string();
        // Reload from the persisted file — same public key.
        let second = CommandSigner::load_or_generate(Some(path_str)).unwrap();
        assert_eq!(second.pubkey_b64(), pubkey_b64);

        // A command signed by the reloaded key verifies against the original pubkey.
        let mut env = envelope(Uuid::now_v7());
        second.sign(&mut env);
        let pubkey = verifying_key_from_b64(&pubkey_b64).unwrap();
        verify_envelope(&pubkey, &env, chrono::Utc::now()).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_routes_commands_per_agent() {
        let queue = CommandQueue::default();
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        queue.enqueue(envelope(a));
        queue.enqueue(envelope(a));
        queue.enqueue(envelope(b));

        let for_a = queue.take_for(a);
        assert_eq!(for_a.len(), 2);
        // Draining is destructive — a second pull is empty.
        assert!(queue.take_for(a).is_empty());
        assert_eq!(queue.take_for(b).len(), 1);
        assert!(queue.take_for(Uuid::now_v7()).is_empty());
    }

    #[test]
    fn queue_records_results() {
        let queue = CommandQueue::default();
        queue.record_result(ActionResult {
            command_id: Uuid::now_v7(),
            status: ActionStatus::Succeeded,
            detail: None,
            observed_state: Some("active".into()),
            finished_at: chrono::Utc::now(),
        });
        assert_eq!(queue.results().len(), 1);
        assert_eq!(queue.results()[0].status, ActionStatus::Succeeded);
    }
}
