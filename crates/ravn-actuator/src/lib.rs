//! The privileged actuator (#113).
//!
//! This is the *only* component that holds privilege on the host. It exposes the
//! fixed [`ravn_core::Capability`] set over a local Unix socket, checks the peer's
//! credentials, **independently re-verifies the signed [`CommandEnvelope`]**
//! (defence-in-depth: even a compromised, unprivileged `ravnd` cannot fabricate a
//! call), executes the typed steps, and reports an [`ActionResult`]. There is no
//! arbitrary-shell path by construction.
//!
//! The execution logic is split behind [`CapabilityExecutor`] so the command
//! handling — verification, validation, step sequencing, post-verify — is unit
//! tested without root or a real socket; the socket/peer-credential wiring in
//! [`serve`] is exercised by the NixOS VM end-to-end test (#121).

use std::path::Path;

use chrono::Utc;
use ravn_core::{ActionResult, ActionStatus, Capability, CommandEnvelope};
use ravn_crypto::{verify_envelope, VerifyingKey};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Executes a single capability. Mutating capabilities return `Ok(None)`;
/// read-only checks return the observed value.
pub trait CapabilityExecutor: Send + Sync {
    fn run(&self, cap: &Capability) -> Result<Option<String>, ExecError>;
}

/// A capability execution failure (non-zero exit, spawn error, invalid input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecError(pub String);

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ExecError {}

/// Validate a systemd unit name before it reaches a subprocess argument.
///
/// Even though arguments are passed directly (never through a shell), we accept
/// only the conservative character set systemd unit names use, so nothing
/// surprising can be smuggled through.
pub fn is_valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 256
        && unit.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | ':' | '\\'))
}

/// The unit a capability targets, if any (all P1 capabilities target a unit).
fn capability_unit(cap: &Capability) -> &str {
    match cap {
        Capability::ResetFailed { unit }
        | Capability::RestartUnit { unit }
        | Capability::UnitState { unit } => unit,
    }
}

/// Handle one command end-to-end: verify the envelope, validate inputs, run the
/// steps in order, then evaluate the verify post-condition. Pure with respect to
/// the injected [`CapabilityExecutor`], so it is fully unit-testable.
pub fn handle_command(
    executor: &dyn CapabilityExecutor,
    key: &VerifyingKey,
    env: &CommandEnvelope,
) -> ActionResult {
    let reject = |detail: String| ActionResult {
        command_id: env.command_id,
        status: ActionStatus::Rejected,
        detail: Some(detail),
        observed_state: None,
        finished_at: Utc::now(),
    };
    let fail = |detail: String, observed: Option<String>| ActionResult {
        command_id: env.command_id,
        status: ActionStatus::Failed,
        detail: Some(detail),
        observed_state: observed,
        finished_at: Utc::now(),
    };

    // 1. Independent signature + expiry verification.
    if let Err(e) = verify_envelope(key, env, Utc::now()) {
        return reject(e.to_string());
    }

    // 2. Validate every targeted unit before doing anything.
    for cap in env.steps.iter().chain(env.verify.as_ref().map(|v| &v.check)) {
        let unit = capability_unit(cap);
        if !is_valid_unit(unit) {
            return reject(format!("invalid unit name: {unit:?}"));
        }
    }

    // 3. Run the steps in order.
    for step in &env.steps {
        if let Err(e) = executor.run(step) {
            return fail(format!("step failed: {e}"), None);
        }
    }

    // 4. Verify post-condition, if declared.
    if let Some(verify) = &env.verify {
        match executor.run(&verify.check) {
            Ok(observed) => {
                let observed = observed.unwrap_or_default();
                if observed != verify.equals {
                    return fail(
                        format!("verify failed: expected {:?}, observed {:?}", verify.equals, observed),
                        Some(observed),
                    );
                }
                return ActionResult {
                    command_id: env.command_id,
                    status: ActionStatus::Succeeded,
                    detail: None,
                    observed_state: Some(observed),
                    finished_at: Utc::now(),
                };
            }
            Err(e) => return fail(format!("verify check errored: {e}"), None),
        }
    }

    ActionResult {
        command_id: env.command_id,
        status: ActionStatus::Succeeded,
        detail: None,
        observed_state: None,
        finished_at: Utc::now(),
    }
}

/// The real executor: drives `systemctl`. Mutating verbs treat a non-zero exit as
/// an error; `is-active` returns the printed state regardless of exit code (it
/// exits non-zero for inactive/failed units while still naming the state).
pub struct SystemctlExecutor;

impl CapabilityExecutor for SystemctlExecutor {
    fn run(&self, cap: &Capability) -> Result<Option<String>, ExecError> {
        use std::process::Command;
        match cap {
            Capability::ResetFailed { unit } => {
                run_ok(Command::new("systemctl").arg("reset-failed").arg(unit))?;
                Ok(None)
            }
            Capability::RestartUnit { unit } => {
                run_ok(Command::new("systemctl").arg("restart").arg(unit))?;
                Ok(None)
            }
            Capability::UnitState { unit } => {
                let out = Command::new("systemctl")
                    .arg("is-active")
                    .arg(unit)
                    .output()
                    .map_err(|e| ExecError(format!("spawning systemctl: {e}")))?;
                Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
            }
        }
    }
}

fn run_ok(cmd: &mut std::process::Command) -> Result<(), ExecError> {
    let out = cmd.output().map_err(|e| ExecError(format!("spawning {cmd:?}: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ExecError(format!(
            "{cmd:?} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// The uid of the process on the other end of a Unix socket, via `SO_PEERCRED`.
fn peer_uid(stream: &UnixStream) -> anyhow::Result<u32> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    let creds = getsockopt(stream, PeerCredentials)?;
    Ok(creds.uid())
}

/// Serve the actuator socket: accept connections, enforce the peer uid, and
/// handle one newline-delimited JSON [`CommandEnvelope`] → [`ActionResult`] per
/// connection. When `allowed_uid` is `Some`, only that uid may connect.
pub async fn serve(
    socket_path: &Path,
    key: VerifyingKey,
    executor: impl CapabilityExecutor + 'static,
    allowed_uid: Option<u32>,
) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o660))?;
    }
    tracing::info!(path = %socket_path.display(), "actuator listening");

    loop {
        let (stream, _addr) = listener.accept().await?;
        match peer_uid(&stream) {
            Ok(uid) if allowed_uid.is_none_or(|allowed| allowed == uid) => {}
            Ok(uid) => {
                tracing::warn!(uid, "rejecting connection from unauthorized peer");
                continue;
            }
            Err(e) => {
                tracing::warn!(%e, "could not read peer credentials; rejecting");
                continue;
            }
        }
        if let Err(e) = handle_connection(stream, &key, &executor).await {
            tracing::warn!(%e, "actuator connection error");
        }
    }
}

async fn handle_connection(
    stream: UnixStream,
    key: &VerifyingKey,
    executor: &dyn CapabilityExecutor,
) -> anyhow::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let env: CommandEnvelope = serde_json::from_str(line.trim())?;
    let result = handle_command(executor, key, &env);
    let mut out = serde_json::to_vec(&result)?;
    out.push(b'\n');
    reader.into_inner().write_all(&out).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use ravn_core::{AgentId, ApprovalRef, RiskTier, Verify};
    use ravn_crypto::{generate_signing_key, sign_envelope, SigningKey};
    use uuid::Uuid;

    /// Records the capabilities run, and returns scripted outputs for checks.
    struct MockExecutor {
        ran: Mutex<Vec<Capability>>,
        unit_state: String,
        fail_on_restart: bool,
    }

    impl MockExecutor {
        fn new(unit_state: &str) -> Self {
            Self { ran: Mutex::new(vec![]), unit_state: unit_state.into(), fail_on_restart: false }
        }
    }

    impl CapabilityExecutor for MockExecutor {
        fn run(&self, cap: &Capability) -> Result<Option<String>, ExecError> {
            self.ran.lock().unwrap().push(cap.clone());
            match cap {
                Capability::RestartUnit { .. } if self.fail_on_restart => {
                    Err(ExecError("unit refused to start".into()))
                }
                Capability::UnitState { .. } => Ok(Some(self.unit_state.clone())),
                _ => Ok(None),
            }
        }
    }

    fn signed_restart_envelope(key: &SigningKey, verify_equals: Option<&str>) -> CommandEnvelope {
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "failed-unit-restart".into(),
            template_version: 3,
            risk_tier: RiskTier::Safe,
            steps: vec![
                Capability::ResetFailed { unit: "nginx.service".into() },
                Capability::RestartUnit { unit: "nginx.service".into() },
            ],
            verify: verify_equals.map(|eq| Verify {
                check: Capability::UnitState { unit: "nginx.service".into() },
                equals: eq.into(),
                timeout_s: 30,
            }),
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "n1".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            sig: None,
        };
        sign_envelope(key, &mut env);
        env
    }

    #[test]
    fn happy_path_runs_steps_and_verifies() {
        let key = generate_signing_key();
        let env = signed_restart_envelope(&key, Some("active"));
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env);
        assert_eq!(result.status, ActionStatus::Succeeded);
        assert_eq!(result.observed_state.as_deref(), Some("active"));
        let ran = exec.ran.lock().unwrap();
        assert_eq!(ran.len(), 3); // reset_failed, restart, unit_state
    }

    #[test]
    fn verify_mismatch_is_failed() {
        let key = generate_signing_key();
        let env = signed_restart_envelope(&key, Some("active"));
        let exec = MockExecutor::new("failed"); // unit did not come back
        let result = handle_command(&exec, &key.verifying_key(), &env);
        assert_eq!(result.status, ActionStatus::Failed);
        assert_eq!(result.observed_state.as_deref(), Some("failed"));
    }

    #[test]
    fn step_failure_is_failed_and_skips_verify() {
        let key = generate_signing_key();
        let env = signed_restart_envelope(&key, Some("active"));
        let mut exec = MockExecutor::new("active");
        exec.fail_on_restart = true;
        let result = handle_command(&exec, &key.verifying_key(), &env);
        assert_eq!(result.status, ActionStatus::Failed);
        // unit_state check must NOT have run after the failed restart.
        let ran = exec.ran.lock().unwrap();
        assert!(!ran.iter().any(|c| matches!(c, Capability::UnitState { .. })));
    }

    #[test]
    fn forged_signature_is_rejected() {
        let key = generate_signing_key();
        let attacker = generate_signing_key();
        let env = signed_restart_envelope(&attacker, Some("active")); // signed by the wrong key
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env);
        assert_eq!(result.status, ActionStatus::Rejected);
        assert!(exec.ran.lock().unwrap().is_empty(), "nothing runs on a bad signature");
    }

    #[test]
    fn invalid_unit_name_is_rejected_before_execution() {
        let key = generate_signing_key();
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "t".into(),
            template_version: 1,
            risk_tier: RiskTier::Safe,
            steps: vec![Capability::RestartUnit { unit: "nginx.service; rm -rf /".into() }],
            verify: None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "n".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            sig: None,
        };
        sign_envelope(&key, &mut env);
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env);
        assert_eq!(result.status, ActionStatus::Rejected);
        assert!(exec.ran.lock().unwrap().is_empty());
    }

    #[test]
    fn valid_unit_names_accepted_bad_ones_rejected() {
        assert!(is_valid_unit("nginx.service"));
        assert!(is_valid_unit("getty@tty1.service"));
        assert!(!is_valid_unit("nginx.service rm -rf"));
        assert!(!is_valid_unit("a/b.service"));
        assert!(!is_valid_unit(""));
    }
}
