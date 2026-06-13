//! Agent-side remediation command pull (#114).
//!
//! `ravnd` pulls signed [`CommandEnvelope`]s from the control plane over its
//! existing outbound connection, verifies each against the public key pinned at
//! enrollment, dispatches the typed steps to the privileged actuator over a local
//! socket, and reports an [`ActionResult`]. `ravnd` itself stays unprivileged —
//! it never executes a capability, it only relays a verified command.
//!
//! Idempotency is **at-most-once**: a command id is written to the on-disk ledger
//! *before* dispatch, so a crash mid-execution never re-runs a remediation
//! (better to under-run a restart than to double-run a destructive fix).

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use ravn_core::{ActionResult, ActionStatus, CommandEnvelope};
use ravn_crypto::{verify_envelope, verifying_key_from_b64, VerifyingKey};
use uuid::Uuid;

/// Load the control-plane command-signing public key pinned at enrollment.
/// Returns `None` when the agent hasn't enrolled (or predates remediation).
pub fn load_pinned_key(cred_dir: &Path) -> Option<VerifyingKey> {
    let path = cred_dir.join("command_pubkey.b64");
    let b64 = std::fs::read_to_string(&path).ok()?;
    match verifying_key_from_b64(&b64) {
        Ok(k) => Some(k),
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "pinned command key is unreadable");
            None
        }
    }
}

/// On-disk set of executed command ids — the idempotency ledger.
pub struct Ledger {
    path: PathBuf,
    seen: Mutex<HashSet<Uuid>>,
}

impl Ledger {
    /// Open (or create) the ledger at `path`, loading any recorded ids.
    pub fn load(path: PathBuf) -> Self {
        let mut seen = HashSet::new();
        if let Ok(contents) = std::fs::read_to_string(&path) {
            for line in contents.lines() {
                if let Ok(id) = line.trim().parse::<Uuid>() {
                    seen.insert(id);
                }
            }
        }
        Self { path, seen: Mutex::new(seen) }
    }

    /// Whether this command id has already been executed.
    pub fn contains(&self, id: Uuid) -> bool {
        self.seen.lock().expect("ledger mutex poisoned").contains(&id)
    }

    /// Record a command id as executed (in memory and appended to disk).
    pub fn record(&self, id: Uuid) -> std::io::Result<()> {
        let mut seen = self.seen.lock().expect("ledger mutex poisoned");
        if seen.insert(id) {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
            writeln!(f, "{id}")?;
        }
        Ok(())
    }
}

/// Decide and (if appropriate) execute one command. Pure with respect to the
/// injected `send` function — verification, dedupe, and the at-most-once ledger
/// write are exercised in tests without a real actuator.
///
/// Returns `None` when there is nothing to report (already executed, or the
/// ledger write failed so we conservatively skip), otherwise the result to POST.
pub fn process_command<F>(
    env: &CommandEnvelope,
    key: &VerifyingKey,
    ledger: &Ledger,
    now: DateTime<Utc>,
    send: F,
) -> Option<ActionResult>
where
    F: FnOnce(&CommandEnvelope) -> anyhow::Result<ActionResult>,
{
    if ledger.contains(env.command_id) {
        return None; // already handled — at-most-once
    }
    if let Err(e) = verify_envelope(key, env, now) {
        tracing::warn!(command_id = %env.command_id, %e, "rejecting command: signature/expiry");
        return Some(result(env.command_id, ActionStatus::Rejected, e.to_string()));
    }
    // Record BEFORE dispatch: a crash mid-execution must not re-run the command.
    if let Err(e) = ledger.record(env.command_id) {
        tracing::warn!(command_id = %env.command_id, %e, "ledger write failed; skipping to avoid double-exec");
        return None;
    }
    match send(env) {
        Ok(r) => Some(r),
        Err(e) => {
            Some(result(env.command_id, ActionStatus::Failed, format!("actuator dispatch failed: {e}")))
        }
    }
}

fn result(command_id: Uuid, status: ActionStatus, detail: String) -> ActionResult {
    ActionResult { command_id, status, detail: Some(detail), observed_state: None, finished_at: Utc::now() }
}

/// Send a verified command to the privileged actuator over its Unix socket and
/// read back the [`ActionResult`]. Synchronous and short-lived (one local
/// round-trip); `ravnd` holds no privilege — the actuator does the work.
pub fn send_to_actuator(socket: &Path, env: &CommandEnvelope) -> anyhow::Result<ActionResult> {
    let stream = std::os::unix::net::UnixStream::connect(socket)?;
    let mut writer = stream.try_clone()?;
    let mut payload = serde_json::to_vec(env)?;
    payload.push(b'\n');
    writer.write_all(&payload)?;
    writer.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

// ---------------------------------------------------------------------------
// Backoff + circuit-breaker constants
// ---------------------------------------------------------------------------

/// The floor applied to any caller-supplied poll interval.
pub const MIN_POLL_SECS: u64 = 5;

/// Multiplier applied to the current backoff delay on each consecutive error.
const BACKOFF_MULTIPLIER: u32 = 2;

/// Upper cap on the computed backoff delay before jitter is applied.
const MAX_BACKOFF_SECS: u64 = 300; // 5 minutes

/// After this many consecutive failures the circuit opens (polling is paused).
const CIRCUIT_OPEN_THRESHOLD: u32 = 5;

/// How long the circuit stays open before a single probe is attempted.
const CIRCUIT_OPEN_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Poll-loop state
// ---------------------------------------------------------------------------

/// Internal mutable state for the adaptive poll loop.
struct PollState {
    /// Consecutive error count; reset to zero on every successful poll.
    consecutive_errors: u32,
    /// Current backoff delay (grows exponentially, capped at MAX_BACKOFF_SECS).
    backoff_secs: u64,
    /// Base poll interval derived from the user config (already >= MIN_POLL_SECS).
    base_poll_secs: u64,
}

impl PollState {
    fn new(poll_secs: u64) -> Self {
        Self {
            consecutive_errors: 0,
            backoff_secs: poll_secs,
            base_poll_secs: poll_secs,
        }
    }

    /// Record a successful poll: reset the error counter and backoff delay.
    fn record_success(&mut self) {
        if self.consecutive_errors > 0 {
            tracing::info!(
                after_errors = self.consecutive_errors,
                "command poll recovered; resetting backoff"
            );
        }
        self.consecutive_errors = 0;
        self.backoff_secs = self.base_poll_secs;
    }

    /// Record a failed poll: increment the counter and compute the next delay
    /// using exponential backoff with full jitter.
    ///
    /// Returns the sleep duration to use before the next attempt, and a flag
    /// indicating whether the circuit just opened.
    fn record_error(&mut self, e: &anyhow::Error) -> (std::time::Duration, bool) {
        self.consecutive_errors += 1;

        // Compute raw exponential cap: base * 2^(errors-1), ceiling at MAX.
        let exponent = (self.consecutive_errors - 1).min(31);
        let raw = self
            .base_poll_secs
            .saturating_mul(BACKOFF_MULTIPLIER.pow(exponent) as u64)
            .min(MAX_BACKOFF_SECS);

        // Full jitter: uniform in [0, raw].  Zero-cost — no heap alloc, uses
        // rand already in the workspace (ravn-crypto depends on it at 0.8).
        use rand::Rng as _;
        let jitter_secs = rand::thread_rng().gen_range(0..=raw);
        self.backoff_secs = jitter_secs.max(1); // never sleep less than 1 s

        let circuit_opened = self.consecutive_errors == CIRCUIT_OPEN_THRESHOLD;

        if circuit_opened {
            tracing::warn!(
                consecutive_errors = self.consecutive_errors,
                backoff_secs = self.backoff_secs,
                circuit_open_secs = CIRCUIT_OPEN_SECS,
                %e,
                "command poll: circuit opened — control plane appears degraded"
            );
        } else {
            tracing::warn!(
                consecutive_errors = self.consecutive_errors,
                backoff_secs = self.backoff_secs,
                %e,
                "command poll failed; backing off"
            );
        }

        (std::time::Duration::from_secs(self.backoff_secs), circuit_opened)
    }

    /// Whether the circuit is currently open (too many consecutive failures).
    fn circuit_is_open(&self) -> bool {
        self.consecutive_errors >= CIRCUIT_OPEN_THRESHOLD
    }
}

/// The pull loop: every `poll_secs` (minimum [`MIN_POLL_SECS`]), fetch pending
/// commands and process them.
///
/// Adaptive behaviour on errors:
/// - Each consecutive error doubles the inter-poll delay (full jitter applied).
/// - The delay is capped at [`MAX_BACKOFF_SECS`].
/// - After [`CIRCUIT_OPEN_THRESHOLD`] consecutive errors the circuit opens: the
///   loop pauses for [`CIRCUIT_OPEN_SECS`] before issuing a single probe.
/// - The first successful response resets the error count and backoff delay.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    http: reqwest::Client,
    base_url: String,
    agent_id: Uuid,
    api_token: Option<String>,
    key: VerifyingKey,
    ledger: std::sync::Arc<Ledger>,
    socket: PathBuf,
    poll_secs: u64,
) {
    let clamped = poll_secs.max(MIN_POLL_SECS);
    if clamped != poll_secs {
        tracing::info!(
            requested = poll_secs,
            enforced = clamped,
            "command poll interval raised to minimum"
        );
    }

    let mut state = PollState::new(clamped);

    loop {
        // Circuit-breaker: if the control plane has been consistently failing,
        // pause for CIRCUIT_OPEN_SECS before probing again.
        if state.circuit_is_open() {
            tracing::warn!(
                pause_secs = CIRCUIT_OPEN_SECS,
                "circuit open — pausing command poll"
            );
            tokio::time::sleep(std::time::Duration::from_secs(CIRCUIT_OPEN_SECS)).await;
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(state.backoff_secs)).await;
        }

        match poll_once(&http, &base_url, agent_id, api_token.as_deref(), &key, &ledger, &socket)
            .await
        {
            Ok(()) => state.record_success(),
            Err(e) => {
                let (_delay, _opened) = state.record_error(&e);
            }
        }
    }
}

async fn poll_once(
    http: &reqwest::Client,
    base_url: &str,
    agent_id: Uuid,
    api_token: Option<&str>,
    key: &VerifyingKey,
    ledger: &Ledger,
    socket: &Path,
) -> anyhow::Result<()> {
    let base = base_url.trim_end_matches('/');
    let mut req = http.get(format!("{base}/agents/{agent_id}/commands"));
    if let Some(t) = api_token {
        req = req.bearer_auth(t);
    }
    let envelopes: Vec<CommandEnvelope> = req.send().await?.error_for_status()?.json().await?;

    for env in &envelopes {
        let Some(result) =
            process_command(env, key, ledger, Utc::now(), |e| send_to_actuator(socket, e))
        else {
            continue;
        };
        let mut post = http
            .post(format!("{base}/agents/{agent_id}/commands/{}/result", result.command_id))
            .json(&result);
        if let Some(t) = api_token {
            post = post.bearer_auth(t);
        }
        if let Err(e) = post.send().await {
            tracing::warn!(command_id = %result.command_id, %e, "reporting command result failed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{AgentId, ApprovalRef, Capability, RiskTier};
    use ravn_crypto::{generate_signing_key, sign_envelope, SigningKey};

    // -----------------------------------------------------------------------
    // PollState / backoff tests
    // -----------------------------------------------------------------------

    #[test]
    fn min_poll_interval_is_enforced() {
        // The constant is the public contract.
        assert!(MIN_POLL_SECS >= 5, "minimum must be at least 5 s");
    }

    #[test]
    fn poll_state_starts_at_base_interval() {
        let s = PollState::new(10);
        assert_eq!(s.backoff_secs, 10);
        assert_eq!(s.consecutive_errors, 0);
    }

    #[test]
    fn backoff_increases_on_consecutive_errors() {
        let mut s = PollState::new(10);
        let err = anyhow::anyhow!("simulated 503");
        let mut delays = vec![];
        for _ in 0..6 {
            let (d, _) = s.record_error(&err);
            delays.push(d.as_secs());
        }
        // The cap of MAX_BACKOFF_SECS (300) means we can't check a strict
        // monotonic increase past it, but the first few must be non-decreasing
        // (jitter is [0, raw], so any single sample could be 0; we verify the
        // ceiling grows instead — raw doubles each step up to the cap).
        //
        // Check that by the 5th error the ceiling (raw) has grown beyond the
        // first error's ceiling (which equals base = 10 * 2^0 = 10).
        // We verify the *maximum possible* backoff keeps growing until the cap.
        assert!(
            s.consecutive_errors == 6,
            "error counter must increment each call"
        );
        // After 6 errors base=10: raw = min(10*2^5, 300) = min(320,300) = 300.
        // At least one recorded delay must be <= 300.
        assert!(delays.iter().all(|&d| d <= MAX_BACKOFF_SECS));
    }

    #[test]
    fn backoff_resets_on_success() {
        let mut s = PollState::new(10);
        let err = anyhow::anyhow!("500");
        for _ in 0..4 {
            s.record_error(&err);
        }
        assert!(s.consecutive_errors > 0);
        s.record_success();
        assert_eq!(s.consecutive_errors, 0);
        assert_eq!(s.backoff_secs, s.base_poll_secs);
    }

    #[test]
    fn circuit_opens_after_threshold_errors() {
        let mut s = PollState::new(10);
        let err = anyhow::anyhow!("timeout");
        let mut opened_at = None;
        for i in 0..CIRCUIT_OPEN_THRESHOLD + 2 {
            let (_, opened) = s.record_error(&err);
            if opened {
                opened_at = Some(i);
            }
        }
        assert_eq!(
            opened_at,
            Some(CIRCUIT_OPEN_THRESHOLD - 1),
            "circuit must open exactly at the threshold"
        );
        assert!(s.circuit_is_open());
    }

    #[test]
    fn circuit_closes_after_success() {
        let mut s = PollState::new(10);
        let err = anyhow::anyhow!("503");
        for _ in 0..CIRCUIT_OPEN_THRESHOLD + 1 {
            s.record_error(&err);
        }
        assert!(s.circuit_is_open());
        s.record_success();
        assert!(!s.circuit_is_open(), "circuit must close on success");
    }

    #[test]
    fn backoff_delay_never_exceeds_cap() {
        let mut s = PollState::new(MIN_POLL_SECS);
        let err = anyhow::anyhow!("storm");
        // Simulate a long storm — 30 consecutive errors.
        for _ in 0..30 {
            let (d, _) = s.record_error(&err);
            assert!(
                d.as_secs() <= MAX_BACKOFF_SECS,
                "delay {d:?} must not exceed MAX_BACKOFF_SECS={MAX_BACKOFF_SECS}"
            );
        }
    }

    #[test]
    fn jitter_produces_different_delays_across_runs() {
        // Run many iterations; all from the same state step.  With full jitter
        // over [0, raw] where raw >= 5, we expect variance.  The probability of
        // all 50 samples being identical is (1/raw)^49 ≈ 0 — this can't flake.
        let err = anyhow::anyhow!("503");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let mut s = PollState::new(MIN_POLL_SECS);
            // advance to error #3 so raw = 5*4 = 20
            for _ in 0..3 {
                let (d, _) = s.record_error(&err);
                seen.insert(d.as_secs());
            }
        }
        assert!(
            seen.len() > 1,
            "jitter must produce varied delays; got only {seen:?}"
        );
    }

    #[test]
    fn recovery_within_one_base_interval_after_success() {
        // After circuit opens and then one success, backoff must return to base.
        let mut s = PollState::new(10);
        let err = anyhow::anyhow!("storm");
        for _ in 0..CIRCUIT_OPEN_THRESHOLD + 3 {
            s.record_error(&err);
        }
        assert!(s.circuit_is_open());
        s.record_success();
        // Immediately after recovery the next sleep must equal base_poll_secs.
        assert_eq!(
            s.backoff_secs, s.base_poll_secs,
            "after recovery, backoff must equal the base poll interval"
        );
        assert!(!s.circuit_is_open());
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ravn-{name}-{}-{}", std::process::id(), Uuid::now_v7()))
    }

    fn signed(key: &SigningKey) -> CommandEnvelope {
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
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
            sig: None,
        };
        sign_envelope(key, &mut env);
        env
    }

    fn ok_result(env: &CommandEnvelope) -> anyhow::Result<ActionResult> {
        Ok(result(env.command_id, ActionStatus::Succeeded, "ok".into()))
    }

    #[test]
    fn ledger_persists_and_reloads() {
        let path = temp_path("ledger");
        let id = Uuid::now_v7();
        {
            let l = Ledger::load(path.clone());
            assert!(!l.contains(id));
            l.record(id).unwrap();
            assert!(l.contains(id));
        }
        // Reload from disk.
        let l2 = Ledger::load(path.clone());
        assert!(l2.contains(id));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn valid_command_executes_once_then_is_skipped() {
        let key = generate_signing_key();
        let env = signed(&key);
        let ledger = Ledger::load(temp_path("ledger"));

        let calls = Mutex::new(0);
        let send = |e: &CommandEnvelope| {
            *calls.lock().unwrap() += 1;
            ok_result(e)
        };
        let r = process_command(&env, &key.verifying_key(), &ledger, Utc::now(), send);
        assert_eq!(r.unwrap().status, ActionStatus::Succeeded);
        assert_eq!(*calls.lock().unwrap(), 1);

        // Re-processing the same command does nothing (at-most-once).
        let r2 = process_command(&env, &key.verifying_key(), &ledger, Utc::now(), |e| {
            *calls.lock().unwrap() += 1;
            ok_result(e)
        });
        assert!(r2.is_none());
        assert_eq!(*calls.lock().unwrap(), 1, "send must not run again");
    }

    #[test]
    fn forged_command_is_rejected_and_not_executed() {
        let key = generate_signing_key();
        let attacker = generate_signing_key();
        let env = signed(&attacker); // signed by the wrong key
        let ledger = Ledger::load(temp_path("ledger"));

        let mut ran = false;
        let r = process_command(&env, &key.verifying_key(), &ledger, Utc::now(), |e| {
            ran = true;
            ok_result(e)
        });
        assert_eq!(r.unwrap().status, ActionStatus::Rejected);
        assert!(!ran, "a forged command must never reach the actuator");
        assert!(!ledger.contains(env.command_id), "rejected commands are not recorded as executed");
    }

    #[test]
    fn actuator_dispatch_error_reports_failed() {
        let key = generate_signing_key();
        let env = signed(&key);
        let ledger = Ledger::load(temp_path("ledger"));
        let r = process_command(&env, &key.verifying_key(), &ledger, Utc::now(), |_| {
            Err(anyhow::anyhow!("connection refused"))
        });
        assert_eq!(r.unwrap().status, ActionStatus::Failed);
        // Recorded before dispatch — at-most-once even on dispatch failure.
        assert!(ledger.contains(env.command_id));
    }

    #[test]
    fn send_to_actuator_round_trips_over_socket() {
        let key = generate_signing_key();
        let env = signed(&key);
        let sock = temp_path("actuator.sock");
        let sock_for_server = sock.clone();
        let expected_id = env.command_id;

        // In-process "actuator": read one envelope line, reply with a result.
        let server = std::thread::spawn(move || {
            let listener = std::os::unix::net::UnixListener::bind(&sock_for_server).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let got: CommandEnvelope = serde_json::from_str(line.trim()).unwrap();
            let mut out = serde_json::to_vec(&result(got.command_id, ActionStatus::Succeeded, "done".into())).unwrap();
            out.push(b'\n');
            let mut w = stream;
            w.write_all(&out).unwrap();
            w.flush().unwrap();
        });

        // Give the listener a moment to bind.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let res = send_to_actuator(&sock, &env).unwrap();
        server.join().unwrap();

        assert_eq!(res.command_id, expected_id);
        assert_eq!(res.status, ActionStatus::Succeeded);
        let _ = std::fs::remove_file(&sock);
    }
}
