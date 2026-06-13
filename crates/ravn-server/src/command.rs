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
    generate_signing_key, key_id, signing_key_from_b64, signing_key_to_b64, verifying_key_from_b64,
    verifying_key_to_b64, Keyring, SigningKey,
};
use uuid::Uuid;

/// Holds the control plane's *active* Ed25519 signing key plus the set of
/// previous keys still trusted during a rotation overlap window (#150).
///
/// The active key signs new commands and is the keyring's default; previous keys
/// only verify (commands they signed while in flight) and are dropped once the
/// overlap window passes. Agents fetch the whole keyring on check-in and pin it,
/// so a key rotates with zero verification failures and no re-enrollment.
pub struct CommandSigner {
    key: SigningKey,
    pubkey_b64: String,
    /// The trusted keyring advertised to agents: the active key (default) plus
    /// any previous keys still inside their overlap window. JSON, precomputed.
    keyring_json: String,
}

impl CommandSigner {
    /// Load the signing key from `path` (base64), generating and persisting one
    /// (mode `0600`) if the file is absent. With `path = None`, generate an
    /// ephemeral key that does not survive a restart (logged as a warning) —
    /// acceptable for dev, never for a real fleet.
    #[allow(dead_code)] // kept as the no-rotation convenience; exercised by tests
    pub fn load_or_generate(path: Option<&str>) -> anyhow::Result<Self> {
        Self::load_or_generate_with_previous(path, None)
    }

    /// As [`Self::load_or_generate`], plus `previous_keys_dir`: a directory of
    /// retiring **public** keys (each file one base64 Ed25519 pubkey) that the
    /// control plane should keep trusting during a rotation overlap window (#150).
    ///
    /// Rotation is then operational, no code change: generate a new signing key,
    /// point `RAVN_COMMAND_KEY` at it, drop the *old* pubkey into this directory,
    /// restart. Agents pick up the new active key and keep trusting the old one
    /// until you remove its file after the window.
    pub fn load_or_generate_with_previous(
        path: Option<&str>,
        previous_keys_dir: Option<&str>,
    ) -> anyhow::Result<Self> {
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

        // The active key is the keyring default (verifies legacy no-kid
        // envelopes and is advertised as the active signing key).
        let active = key.verifying_key();
        let mut ring = Keyring::single(active);
        let active_kid = key_id(&active);

        if let Some(dir) = previous_keys_dir {
            for (file, b64) in read_previous_pubkeys(dir)? {
                match verifying_key_from_b64(&b64) {
                    Ok(prev) => {
                        let kid = ring.insert(prev);
                        if kid != active_kid {
                            tracing::info!(%kid, "trusting previous command key during rotation overlap");
                        }
                    }
                    Err(e) => {
                        anyhow::bail!("invalid previous command key in {file}: {e}");
                    }
                }
            }
        }
        ring.set_default(&active_kid);
        let keyring_json = ring.to_json();

        Ok(Self { key, pubkey_b64, keyring_json })
    }

    /// Sign an envelope in place. Called by the orchestrator (#115) when a
    /// remediation is approved/auto-approved. Stamps the active key's `kid`.
    #[allow(dead_code)] // wired in by the orchestrator (#115)
    pub fn sign(&self, env: &mut CommandEnvelope) {
        ravn_crypto::sign_envelope(&self.key, env);
    }

    /// The base64 *active* public key advertised to agents at enrollment.
    /// (Backward-compat: pre-#150 agents pin just this one key.)
    pub fn pubkey_b64(&self) -> &str {
        &self.pubkey_b64
    }

    /// The full trusted keyring as a JSON document (#150): the active key plus
    /// any previous keys inside their overlap window. Agents fetch this on
    /// check-in and pin it, so a rotation needs no re-enrollment.
    pub fn keyring_json(&self) -> &str {
        &self.keyring_json
    }
}

/// Read every regular file in `dir` as a base64 public key, returning
/// `(filename, contents)` pairs. A missing directory is not an error (no
/// previous keys configured); other IO errors propagate.
fn read_previous_pubkeys(dir: &str) -> anyhow::Result<Vec<(String, String)>> {
    let path = std::path::Path::new(dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(path).with_context(|| format!("reading previous keys dir {dir}"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let contents = std::fs::read_to_string(entry.path())
                .with_context(|| format!("reading previous key {name}"))?;
            out.push((name, contents));
        }
    }
    Ok(out)
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
            preconditions: vec![],
            steps: vec![Capability::RestartUnit { unit: "nginx.service".into() }],
            verify: None,
            rollback: ravn_core::Rollback::None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "n".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            kid: None,
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
    fn keyring_advertises_active_plus_previous_keys() {
        use ravn_crypto::{key_id, signing_key_to_b64, verifying_key_to_b64, Keyring};

        let dir = std::env::temp_dir().join(format!("ravn-prevkeys-{}", Uuid::now_v7()));
        // Keep the active signing key and the previous-keys directory separate:
        // every file in the previous-keys dir is treated as a trusted pubkey.
        let prev_dir = dir.join("previous");
        std::fs::create_dir_all(&prev_dir).unwrap();

        // An "old" key to retire, written into the previous-keys dir as a pubkey.
        let old = generate_signing_key();
        std::fs::write(prev_dir.join("old.b64"), verifying_key_to_b64(&old.verifying_key())).unwrap();

        // A fresh active signing key persisted at its own path (outside prev_dir).
        let active = generate_signing_key();
        let keypath = dir.join("active.key");
        std::fs::write(&keypath, signing_key_to_b64(&active)).unwrap();

        let signer = CommandSigner::load_or_generate_with_previous(
            Some(keypath.to_str().unwrap()),
            Some(prev_dir.to_str().unwrap()),
        )
        .unwrap();

        let ring = Keyring::from_json(signer.keyring_json()).unwrap();
        assert_eq!(ring.len(), 2, "ring holds active + previous");
        assert_eq!(ring.default_kid(), Some(key_id(&active.verifying_key()).as_str()));
        assert!(ring.get(&key_id(&old.verifying_key())).is_some(), "old key still trusted");

        // A command signed by the active key carries the active kid and verifies
        // against the advertised ring.
        let mut env = envelope(Uuid::now_v7());
        signer.sign(&mut env);
        assert_eq!(env.kid.as_deref(), Some(key_id(&active.verifying_key()).as_str()));
        ravn_crypto::verify_envelope_with(&ring, &env, chrono::Utc::now()).unwrap();

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
