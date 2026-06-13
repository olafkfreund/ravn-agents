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
//!
//! # Concurrency model (fix #144)
//!
//! Each accepted connection is handled on its own `tokio::spawn`'d task, bounded
//! by a shared [`tokio::sync::Semaphore`]. This means a long-running verify-poll
//! on one connection cannot starve any other inbound command.
//!
//! The verify post-condition is polled asynchronously with
//! `tokio::time::sleep` (500 ms interval) up to the envelope's `timeout_s` budget,
//! so there is **no `std::thread::sleep` in the command path**.
//!
//! The real [`SystemctlExecutor`] uses `tokio::task::spawn_blocking` to offload the
//! blocking `std::process::Command` calls to the dedicated blocking-thread pool,
//! eliminating any `block_in_place` hazard on a shared runtime.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use ravn_core::{
    is_valid_k8s_label, ActionResult, ActionStatus, Capability, CommandEnvelope, Rollback,
};
use ravn_crypto::{verify_envelope, VerifyingKey};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

/// Maximum number of in-flight command connections handled concurrently.
const MAX_CONCURRENT_COMMANDS: usize = 64;

/// Polling interval used when waiting for a verify post-condition to become true.
const VERIFY_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Boxed, pinned future returned by [`CapabilityExecutor::run`]. Using a boxed
/// future makes the trait dyn-compatible so callers can accept
/// `&dyn CapabilityExecutor` without monomorphising the entire call chain.
pub type ExecFuture<'a> =
    Pin<Box<dyn std::future::Future<Output = Result<Option<String>, ExecError>> + Send + 'a>>;

/// Executes a single capability asynchronously. Mutating capabilities return
/// `Ok(None)`; read-only checks return the observed value.
///
/// Implementors that call blocking system APIs (e.g. `systemctl`) must
/// offload the blocking work to [`tokio::task::spawn_blocking`] rather than
/// calling it directly on the async runtime thread.
pub trait CapabilityExecutor: Send + Sync {
    fn run<'a>(&'a self, cap: &'a Capability) -> ExecFuture<'a>;
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
        && unit
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | ':' | '\\'))
}

/// The unit a capability targets, if any. `nix_rollback` targets no unit.
fn capability_unit(cap: &Capability) -> Option<&str> {
    match cap {
        Capability::ResetFailed { unit }
        | Capability::RestartUnit { unit }
        | Capability::UnitState { unit } => Some(unit),
        Capability::NixRollback
        | Capability::DeletePod { .. }
        | Capability::RestartDeployment { .. }
        | Capability::PodState { .. } => None,
    }
}

/// Kubernetes identifiers targeted by a capability, as `(field_name, value)` pairs.
/// Returns an empty slice for capabilities that carry no K8s identifiers.
fn capability_kube_ids(cap: &Capability) -> Vec<(&'static str, &str)> {
    match cap {
        Capability::DeletePod { namespace, pod } => {
            vec![("namespace", namespace.as_str()), ("pod", pod.as_str())]
        }
        Capability::RestartDeployment { namespace, deployment } => {
            vec![("namespace", namespace.as_str()), ("deployment", deployment.as_str())]
        }
        Capability::PodState { namespace, pod_prefix } => {
            vec![("namespace", namespace.as_str()), ("pod_prefix", pod_prefix.as_str())]
        }
        _ => Vec::new(),
    }
}

/// Whether any step or check in the envelope is a Kubernetes capability. The host
/// actuator cannot execute these — they belong to the in-cluster executor (#146),
/// which calls [`handle_command`] directly. The host actuator therefore rejects
/// such envelopes at its socket boundary (see [`handle_connection`]).
fn envelope_contains_kube_capability(env: &CommandEnvelope) -> bool {
    let is_kube = |c: &Capability| {
        matches!(
            c,
            Capability::DeletePod { .. }
                | Capability::RestartDeployment { .. }
                | Capability::PodState { .. }
        )
    };
    env.preconditions.iter().any(|c| is_kube(&c.check))
        || env.steps.iter().any(is_kube)
        || env.verify.as_ref().is_some_and(|v| is_kube(&v.check))
}

/// Handle one command end-to-end: verify the envelope, validate inputs, check
/// the preconditions, run the steps in order, poll the verify post-condition
/// asynchronously, and — if verify fails — perform the declared rollback. Pure
/// with respect to the injected [`CapabilityExecutor`], so it is fully
/// unit-testable.
///
/// The verify post-condition is polled every [`VERIFY_POLL_INTERVAL`] until
/// either the expected state is observed or the envelope's `timeout_s` budget
/// is exhausted — using `tokio::time::sleep` with no blocking thread involved.
pub async fn handle_command(
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
    let make_result = |status: ActionStatus, detail: Option<String>, observed: Option<String>| {
        ActionResult {
            command_id: env.command_id,
            status,
            detail,
            observed_state: observed,
            finished_at: Utc::now(),
        }
    };
    let fail = |detail: String, observed: Option<String>| {
        make_result(ActionStatus::Failed, Some(detail), observed)
    };

    // 1. Independent signature + expiry verification.
    if let Err(e) = verify_envelope(key, env, Utc::now()) {
        return reject(e.to_string());
    }

    // 2. Validate every targeted identifier before doing anything (preconditions,
    //    steps, and the verify check). Covers both systemd unit names and
    //    Kubernetes RFC 1123 identifiers; `nix_rollback` targets nothing.
    let precondition_checks = env.preconditions.iter().map(|c| &c.check);
    let verify_check = env.verify.as_ref().map(|v| &v.check);
    for cap in precondition_checks.chain(env.steps.iter()).chain(verify_check) {
        if let Some(unit) = capability_unit(cap) {
            if !is_valid_unit(unit) {
                return reject(format!("invalid unit name: {unit:?}"));
            }
        }
        for (field, value) in capability_kube_ids(cap) {
            if !is_valid_k8s_label(value) {
                return reject(format!("invalid Kubernetes identifier for {field}: {value:?}"));
            }
        }
    }

    // 3. Check preconditions BEFORE running any step; a failed precondition
    //    means the world is not in the expected state, so we do nothing (#117).
    for cond in &env.preconditions {
        match executor.run(&cond.check).await {
            Ok(observed) => {
                let observed = observed.unwrap_or_default();
                if observed != cond.equals {
                    return make_result(
                        ActionStatus::PreconditionFailed,
                        Some(format!(
                            "precondition failed: expected {:?}, observed {:?}",
                            cond.equals, observed
                        )),
                        Some(observed),
                    );
                }
            }
            Err(e) => return fail(format!("precondition check errored: {e}"), None),
        }
    }

    // 4. Run the steps in order.
    for step in &env.steps {
        if let Err(e) = executor.run(step).await {
            return fail(format!("step failed: {e}"), None);
        }
    }

    // 5. Verify post-condition, if declared. Poll asynchronously up to the
    //    envelope's `timeout_s` budget with VERIFY_POLL_INTERVAL ticks.
    //    On final failure, perform the declared rollback before reporting (#117).
    if let Some(verify) = &env.verify {
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(verify.timeout_s);

        loop {
            match executor.run(&verify.check).await {
                Ok(observed) => {
                    let observed = observed.unwrap_or_default();
                    if observed == verify.equals {
                        return make_result(ActionStatus::Succeeded, None, Some(observed));
                    }
                    // Not yet the desired state — check whether we have time left.
                    if tokio::time::Instant::now() >= deadline {
                        let detail = format!(
                            "verify failed: expected {:?}, observed {:?}",
                            verify.equals, observed
                        );
                        return perform_rollback(executor, env, detail, observed).await;
                    }
                    tokio::time::sleep(VERIFY_POLL_INTERVAL).await;
                }
                Err(e) => {
                    let detail = format!("verify check errored: {e}");
                    return perform_rollback(executor, env, detail, String::new()).await;
                }
            }
        }
    }

    make_result(ActionStatus::Succeeded, None, None)
}

/// Verification has failed; perform the envelope's declared rollback. If there is
/// no rollback the outcome is plain [`ActionStatus::Failed`]; if a rollback runs
/// but itself fails, the target is left in an unknown state and we return
/// [`ActionStatus::Frozen`] so upstream stops auto-acting on it (#117).
async fn perform_rollback(
    executor: &dyn CapabilityExecutor,
    env: &CommandEnvelope,
    verify_detail: String,
    observed: String,
) -> ActionResult {
    let observed = (!observed.is_empty()).then_some(observed);
    let finished = |status: ActionStatus, detail: String| ActionResult {
        command_id: env.command_id,
        status,
        detail: Some(detail),
        observed_state: observed.clone(),
        finished_at: Utc::now(),
    };

    match env.rollback {
        Rollback::None => finished(ActionStatus::Failed, verify_detail),
        Rollback::NixGeneration => match executor.run(&Capability::NixRollback).await {
            Ok(_) => finished(
                ActionStatus::Failed,
                format!("{verify_detail}; rolled back to the previous NixOS generation"),
            ),
            Err(e) => finished(
                ActionStatus::Frozen,
                format!(
                    "{verify_detail}; rollback FAILED ({e}) — target frozen, escalate to a human"
                ),
            ),
        },
    }
}

/// The real executor: drives `systemctl` via [`tokio::task::spawn_blocking`] so
/// the blocking `std::process::Command` calls are offloaded to the dedicated
/// blocking-thread pool and never stall the async runtime.
///
/// Mutating verbs treat a non-zero exit as an error; `is-active` returns the
/// printed state regardless of exit code (it exits non-zero for inactive/failed
/// units while still naming the state).
///
/// Kubernetes capabilities ([`Capability::DeletePod`],
/// [`Capability::RestartDeployment`], [`Capability::PodState`]) are not handled
/// by this executor — they require a separate in-cluster executor that holds a
/// kubeconfig or ServiceAccount token. The host actuator rejects such envelopes
/// at its socket boundary, so reaching these arms is a programmer error and
/// returns an [`ExecError`].
pub struct SystemctlExecutor;

impl CapabilityExecutor for SystemctlExecutor {
    fn run<'a>(&'a self, cap: &'a Capability) -> ExecFuture<'a> {
        // Clone the capability so it can be moved into the blocking closure.
        let cap = cap.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run_systemctl_sync(&cap))
                .await
                .map_err(|e| ExecError(format!("spawn_blocking panicked: {e}")))?
        })
    }
}

/// The synchronous systemctl implementation, called from within `spawn_blocking`.
fn run_systemctl_sync(cap: &Capability) -> Result<Option<String>, ExecError> {
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
        // Roll the system back to its previous NixOS generation, then activate
        // it — the universal safety net. Two direct-argv steps, no shell. Not
        // hermetically unit-testable (needs a real NixOS host); the rollback
        // *logic* in `handle_command` is covered via the MockExecutor.
        Capability::NixRollback => {
            const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";
            run_ok(Command::new("nix-env").arg("--rollback").arg("-p").arg(SYSTEM_PROFILE))?;
            run_ok(Command::new(concat!(
                "/nix/var/nix/profiles/system",
                "/bin/switch-to-configuration"
            ))
            .arg("switch"))?;
            Ok(None)
        }
        // Kubernetes capabilities require a separate in-cluster executor; they
        // must never reach the systemd executor on a non-K8s host.
        Capability::DeletePod { namespace, pod } => Err(ExecError(format!(
            "DeletePod ({namespace}/{pod}) reached systemctl executor — \
             K8s capabilities require an in-cluster executor"
        ))),
        Capability::RestartDeployment { namespace, deployment } => Err(ExecError(format!(
            "RestartDeployment ({namespace}/{deployment}) reached systemctl executor — \
             K8s capabilities require an in-cluster executor"
        ))),
        Capability::PodState { namespace, pod_prefix } => Err(ExecError(format!(
            "PodState ({namespace}/{pod_prefix}) reached systemctl executor — \
             K8s capabilities require an in-cluster executor"
        ))),
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
/// connection.
///
/// Each accepted connection is spawned onto its own Tokio task (bounded by
/// [`MAX_CONCURRENT_COMMANDS`]). A long-running verify-poll on one connection
/// **cannot** starve any other inbound command.
///
/// When `allowed_uid` is `Some`, only that uid may connect.
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

    let executor = Arc::new(executor);
    let key = Arc::new(key);
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_COMMANDS));

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

        let executor = Arc::clone(&executor);
        let key = Arc::clone(&key);
        // Acquire a permit before spawning — back-pressures callers if all
        // MAX_CONCURRENT_COMMANDS slots are busy, without blocking the accept loop.
        let permit = Arc::clone(&semaphore).acquire_owned().await?;

        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, &key, executor.as_ref()).await {
                tracing::warn!(%e, "actuator connection error");
            }
            drop(permit); // release the semaphore slot when done
        });
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
    // The host actuator cannot execute Kubernetes capabilities — those belong to
    // the in-cluster executor (#146), which invokes `handle_command` directly.
    // Reject at this boundary rather than inside `handle_command` so the K8s
    // executor's own path is never short-circuited.
    let result = if envelope_contains_kube_capability(&env) {
        ActionResult {
            command_id: env.command_id,
            status: ActionStatus::Rejected,
            detail: Some(
                "command contains Kubernetes capabilities; route to the in-cluster K8s executor (#146)"
                    .into(),
            ),
            observed_state: None,
            finished_at: Utc::now(),
        }
    } else {
        handle_command(executor, key, &env).await
    };
    let mut out = serde_json::to_vec(&result)?;
    out.push(b'\n');
    reader.into_inner().write_all(&out).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use ravn_core::{AgentId, ApprovalRef, Condition, RiskTier, Rollback, Verify};
    use ravn_crypto::{generate_signing_key, sign_envelope, SigningKey};
    use uuid::Uuid;

    /// Records the capabilities run, and returns scripted outputs for checks.
    struct MockExecutor {
        ran: Mutex<Vec<Capability>>,
        unit_state: String,
        fail_on_restart: bool,
        fail_on_rollback: bool,
    }

    impl MockExecutor {
        fn new(unit_state: &str) -> Self {
            Self {
                ran: Mutex::new(vec![]),
                unit_state: unit_state.into(),
                fail_on_restart: false,
                fail_on_rollback: false,
            }
        }
    }

    impl CapabilityExecutor for MockExecutor {
        fn run<'a>(&'a self, cap: &'a Capability) -> ExecFuture<'a> {
            // Push to ran synchronously before returning the future so the
            // ordering is deterministic regardless of await scheduling.
            self.ran.lock().unwrap().push(cap.clone());
            let unit_state = self.unit_state.clone();
            let fail_on_restart = self.fail_on_restart;
            let fail_on_rollback = self.fail_on_rollback;
            let cap = cap.clone();
            Box::pin(async move {
                match &cap {
                    Capability::RestartUnit { .. } if fail_on_restart => {
                        Err(ExecError("unit refused to start".into()))
                    }
                    Capability::NixRollback if fail_on_rollback => {
                        Err(ExecError("switch-to-configuration failed".into()))
                    }
                    Capability::UnitState { .. } => Ok(Some(unit_state)),
                    _ => Ok(None),
                }
            })
        }
    }

    /// A signed `reset_failed` + `restart_unit` envelope, optionally with a verify
    /// post-condition, a precondition, and a declared rollback.
    struct EnvelopeOpts<'a> {
        verify_equals: Option<&'a str>,
        precondition_equals: Option<&'a str>,
        rollback: Rollback,
        /// Verify timeout in seconds (kept short in tests so polling exits fast).
        verify_timeout_s: u64,
    }

    impl Default for EnvelopeOpts<'_> {
        fn default() -> Self {
            Self {
                verify_equals: None,
                precondition_equals: None,
                rollback: Rollback::None,
                verify_timeout_s: 1,
            }
        }
    }

    fn signed_restart_envelope(key: &SigningKey, verify_equals: Option<&str>) -> CommandEnvelope {
        signed_envelope(key, EnvelopeOpts { verify_equals, ..Default::default() })
    }

    fn signed_envelope(key: &SigningKey, opts: EnvelopeOpts<'_>) -> CommandEnvelope {
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "failed-unit-restart".into(),
            template_version: 3,
            risk_tier: RiskTier::Safe,
            preconditions: opts
                .precondition_equals
                .map(|eq| {
                    vec![Condition {
                        check: Capability::UnitState { unit: "nginx.service".into() },
                        equals: eq.into(),
                    }]
                })
                .unwrap_or_default(),
            steps: vec![
                Capability::ResetFailed { unit: "nginx.service".into() },
                Capability::RestartUnit { unit: "nginx.service".into() },
            ],
            verify: opts.verify_equals.map(|eq| Verify {
                check: Capability::UnitState { unit: "nginx.service".into() },
                equals: eq.into(),
                timeout_s: opts.verify_timeout_s,
            }),
            rollback: opts.rollback,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "n1".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            sig: None,
        };
        sign_envelope(key, &mut env);
        env
    }

    #[tokio::test]
    async fn happy_path_runs_steps_and_verifies() {
        let key = generate_signing_key();
        let env = signed_restart_envelope(&key, Some("active"));
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Succeeded);
        assert_eq!(result.observed_state.as_deref(), Some("active"));
        let ran = exec.ran.lock().unwrap();
        assert_eq!(ran.len(), 3); // reset_failed, restart, unit_state
    }

    #[tokio::test]
    async fn verify_mismatch_is_failed() {
        let key = generate_signing_key();
        let env = signed_restart_envelope(&key, Some("active"));
        let exec = MockExecutor::new("failed"); // unit did not come back
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Failed);
        assert_eq!(result.observed_state.as_deref(), Some("failed"));
    }

    #[tokio::test]
    async fn step_failure_is_failed_and_skips_verify() {
        let key = generate_signing_key();
        let env = signed_restart_envelope(&key, Some("active"));
        let mut exec = MockExecutor::new("active");
        exec.fail_on_restart = true;
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Failed);
        // unit_state check must NOT have run after the failed restart.
        let ran = exec.ran.lock().unwrap();
        assert!(!ran.iter().any(|c| matches!(c, Capability::UnitState { .. })));
    }

    #[tokio::test]
    async fn forged_signature_is_rejected() {
        let key = generate_signing_key();
        let attacker = generate_signing_key();
        let env = signed_restart_envelope(&attacker, Some("active")); // signed by the wrong key
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected);
        assert!(exec.ran.lock().unwrap().is_empty(), "nothing runs on a bad signature");
    }

    #[tokio::test]
    async fn invalid_unit_name_is_rejected_before_execution() {
        let key = generate_signing_key();
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "t".into(),
            template_version: 1,
            risk_tier: RiskTier::Safe,
            preconditions: vec![],
            steps: vec![Capability::RestartUnit { unit: "nginx.service; rm -rf /".into() }],
            verify: None,
            rollback: Rollback::None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "n".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            sig: None,
        };
        sign_envelope(&key, &mut env);
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
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

    // ── K8s identifier validation in handle_command (issue #145) ─────────

    fn signed_kube_pod_restart_envelope(
        key: &SigningKey,
        namespace: &str,
        pod: &str,
    ) -> CommandEnvelope {
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "k8s-pod-restart".into(),
            template_version: 1,
            risk_tier: RiskTier::Guarded,
            preconditions: vec![],
            steps: vec![Capability::DeletePod {
                namespace: namespace.into(),
                pod: pod.into(),
            }],
            verify: None,
            rollback: Rollback::None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "k8s-nonce".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            sig: None,
        };
        sign_envelope(key, &mut env);
        env
    }

    #[tokio::test]
    async fn valid_k8s_pod_restart_passes_validation() {
        let key = generate_signing_key();
        let env = signed_kube_pod_restart_envelope(&key, "payments", "api-7d9f");
        let exec = MockExecutor::new("active");
        // `handle_command` itself does not reject K8s capabilities (the in-cluster
        // executor relies on it). The MockExecutor returns Ok(None) for DeletePod,
        // so a well-formed identifier passes validation and is not rejected.
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_ne!(result.status, ActionStatus::Rejected, "valid K8s identifiers must pass");
    }

    #[test]
    fn host_actuator_guard_flags_kube_capabilities() {
        let key = generate_signing_key();
        // An envelope with a K8s step is flagged for host-actuator rejection.
        let kube = signed_kube_pod_restart_envelope(&key, "payments", "api-7d9f");
        assert!(envelope_contains_kube_capability(&kube));
        // A systemd envelope is not.
        let systemd = signed_envelope(&key, EnvelopeOpts::default());
        assert!(!envelope_contains_kube_capability(&systemd));
    }

    #[tokio::test]
    async fn injection_namespace_is_rejected_before_execution() {
        let key = generate_signing_key();
        let env = signed_kube_pod_restart_envelope(&key, "payments; rm -rf /", "api");
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected, "injection namespace must be rejected");
        assert!(exec.ran.lock().unwrap().is_empty(), "no step must run on rejection");
    }

    #[tokio::test]
    async fn uppercase_namespace_is_rejected() {
        let key = generate_signing_key();
        let env = signed_kube_pod_restart_envelope(&key, "Payments", "api");
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected);
        assert!(result.detail.as_deref().unwrap().contains("namespace"));
    }

    #[tokio::test]
    async fn path_traversal_pod_name_is_rejected() {
        let key = generate_signing_key();
        let env = signed_kube_pod_restart_envelope(&key, "payments", "../etc/passwd");
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected);
        assert!(result.detail.as_deref().unwrap().contains("pod"));
    }

    #[tokio::test]
    async fn empty_namespace_is_rejected() {
        let key = generate_signing_key();
        let env = signed_kube_pod_restart_envelope(&key, "", "api");
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected);
    }

    #[tokio::test]
    async fn overly_long_pod_name_is_rejected() {
        let key = generate_signing_key();
        let long_name = "a".repeat(64);
        let env = signed_kube_pod_restart_envelope(&key, "payments", &long_name);
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected);
    }

    #[tokio::test]
    async fn whitespace_in_k8s_id_is_rejected() {
        let key = generate_signing_key();
        let env = signed_kube_pod_restart_envelope(&key, "pay ments", "api");
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Rejected);
    }

    #[tokio::test]
    async fn failed_precondition_skips_all_steps() {
        let key = generate_signing_key();
        // Precondition demands "failed", but the unit is already "active": the
        // remediation is unnecessary, so nothing should run.
        let env = signed_envelope(
            &key,
            EnvelopeOpts { precondition_equals: Some("failed"), ..Default::default() },
        );
        let exec = MockExecutor::new("active");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::PreconditionFailed);
        assert_eq!(result.observed_state.as_deref(), Some("active"));
        let ran = exec.ran.lock().unwrap();
        // Only the precondition check ran — no reset_failed/restart.
        assert_eq!(ran.len(), 1);
        assert!(matches!(ran[0], Capability::UnitState { .. }));
    }

    #[tokio::test]
    async fn met_precondition_runs_steps() {
        let key = generate_signing_key();
        let env = signed_envelope(
            &key,
            EnvelopeOpts {
                precondition_equals: Some("failed"),
                verify_equals: Some("active"),
                ..Default::default()
            },
        );
        let exec = MockExecutor::new("failed"); // precondition holds; verify will see this too
        // Precondition sees "failed" (holds). After restart, verify also reads the
        // mock's fixed state "failed", so verify fails — but the precondition phase
        // itself must have let the steps run.
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        let ran = exec.ran.lock().unwrap();
        assert!(ran.iter().any(|c| matches!(c, Capability::RestartUnit { .. })));
        assert_ne!(result.status, ActionStatus::PreconditionFailed);
    }

    #[tokio::test]
    async fn verify_failure_triggers_nix_rollback() {
        let key = generate_signing_key();
        let env = signed_envelope(
            &key,
            EnvelopeOpts {
                verify_equals: Some("active"),
                rollback: Rollback::NixGeneration,
                ..Default::default()
            },
        );
        let exec = MockExecutor::new("failed"); // unit never recovers → verify fails
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        // Rollback ran and succeeded, so the outcome is Failed (not Frozen).
        assert_eq!(result.status, ActionStatus::Failed);
        let ran = exec.ran.lock().unwrap();
        assert!(ran.iter().any(|c| matches!(c, Capability::NixRollback)));
        assert!(result.detail.as_deref().unwrap().contains("rolled back"));
    }

    #[tokio::test]
    async fn rollback_failure_freezes_the_target() {
        let key = generate_signing_key();
        let env = signed_envelope(
            &key,
            EnvelopeOpts {
                verify_equals: Some("active"),
                rollback: Rollback::NixGeneration,
                ..Default::default()
            },
        );
        let mut exec = MockExecutor::new("failed"); // verify fails
        exec.fail_on_rollback = true; // and the rollback itself fails
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Frozen);
        assert!(result.detail.as_deref().unwrap().contains("frozen"));
    }

    #[tokio::test]
    async fn verify_failure_without_rollback_is_plain_failed() {
        let key = generate_signing_key();
        // Default rollback is None.
        let env = signed_restart_envelope(&key, Some("active"));
        let exec = MockExecutor::new("failed");
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Failed);
        let ran = exec.ran.lock().unwrap();
        assert!(!ran.iter().any(|c| matches!(c, Capability::NixRollback)));
    }

    // -------------------------------------------------------------------------
    // Issue #144 — concurrency test
    //
    // Prove that a second command executes to completion while the first is
    // still in its verify-polling loop. We use two executors:
    //
    //   • `SlowVerifyExecutor` — the verify check always returns the wrong
    //     state, so `handle_command` keeps polling until `timeout_s` expires.
    //     The timeout is set to 2 s so the test stays fast but gives clear
    //     headroom.
    //
    //   • `FastExecutor` — no verify; completes immediately.
    //
    // Both tasks are spawned concurrently. If the async path were still
    // blocking (e.g. `std::thread::sleep`), the fast task would not complete
    // until the slow one timed out. We assert the fast task finishes well
    // before the slow task's deadline.
    // -------------------------------------------------------------------------

    /// Always returns the wrong unit state so verify polling runs until timeout.
    struct SlowVerifyExecutor;

    impl CapabilityExecutor for SlowVerifyExecutor {
        fn run<'a>(&'a self, _cap: &'a Capability) -> ExecFuture<'a> {
            // Simulate a tiny async delay without blocking the thread.
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(Some("failed".to_string())) // always "failed" → verify never passes
            })
        }
    }

    /// Completes immediately with no verify step.
    struct FastExecutor;

    impl CapabilityExecutor for FastExecutor {
        fn run<'a>(&'a self, _cap: &'a Capability) -> ExecFuture<'a> {
            Box::pin(async { Ok(None) })
        }
    }

    fn make_slow_envelope(key: &SigningKey) -> CommandEnvelope {
        // verify_timeout_s = 2 s, verify always fails → polling runs for ~2 s.
        signed_envelope(
            key,
            EnvelopeOpts {
                verify_equals: Some("active"), // SlowVerifyExecutor returns "failed" → mismatch
                verify_timeout_s: 2,
                ..Default::default()
            },
        )
    }

    fn make_fast_envelope(key: &SigningKey) -> CommandEnvelope {
        // No verify step — finishes as soon as the two steps succeed.
        signed_envelope(key, EnvelopeOpts::default())
    }

    #[tokio::test]
    async fn second_command_completes_while_first_is_in_verify_polling() {
        let key = generate_signing_key();
        let slow_env = make_slow_envelope(&key);
        let fast_env = make_fast_envelope(&key);
        let vk = key.verifying_key();

        let start = std::time::Instant::now();

        // Spawn both concurrently. The slow task polls verify for ~2 s.
        // The fast task should finish in well under 500 ms.
        let slow_handle = tokio::spawn({
            let vk = vk.clone();
            async move { handle_command(&SlowVerifyExecutor, &vk, &slow_env).await }
        });

        let fast_handle = tokio::spawn({
            let vk = vk.clone();
            async move { handle_command(&FastExecutor, &vk, &fast_env).await }
        });

        // Wait for the fast task first with a tight deadline well under the
        // slow task's 2 s timeout.
        let fast_result = tokio::time::timeout(Duration::from_millis(800), fast_handle)
            .await
            .expect("fast command timed out — the async path is blocking!")
            .expect("task panicked");

        let fast_elapsed = start.elapsed();

        // Fast command must have completed successfully in under 800 ms.
        assert_eq!(
            fast_result.status,
            ActionStatus::Succeeded,
            "fast command should succeed"
        );
        assert!(
            fast_elapsed < Duration::from_millis(800),
            "fast command took {fast_elapsed:?}, expected < 800 ms"
        );

        // Now wait for the slow one to finish naturally.
        let slow_result = slow_handle.await.expect("slow task panicked");
        assert_eq!(
            slow_result.status,
            ActionStatus::Failed,
            "slow command should fail after verify timeout"
        );
    }

    /// Verify polling eventually succeeds when the executor transitions state.
    ///
    /// The executor returns "activating" for the first two checks, then "active".
    /// `handle_command` must succeed once the correct state is observed.
    #[tokio::test]
    async fn verify_polling_succeeds_after_state_transition() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct TransitioningExecutor {
            check_count: AtomicUsize,
        }

        impl CapabilityExecutor for TransitioningExecutor {
            fn run<'a>(&'a self, cap: &'a Capability) -> ExecFuture<'a> {
                let n = self.check_count.fetch_add(1, Ordering::SeqCst);
                let is_state_check = matches!(cap, Capability::UnitState { .. });
                Box::pin(async move {
                    if is_state_check {
                        // First two checks return "activating"; third returns "active".
                        if n < 2 {
                            Ok(Some("activating".to_string()))
                        } else {
                            Ok(Some("active".to_string()))
                        }
                    } else {
                        Ok(None)
                    }
                })
            }
        }

        let key = generate_signing_key();
        // Use a 5-second timeout — plenty of room for 2-3 polls at 500 ms each.
        let env = signed_envelope(
            &key,
            EnvelopeOpts {
                verify_equals: Some("active"),
                verify_timeout_s: 5,
                ..Default::default()
            },
        );
        let exec = TransitioningExecutor { check_count: AtomicUsize::new(0) };
        let result = handle_command(&exec, &key.verifying_key(), &env).await;
        assert_eq!(result.status, ActionStatus::Succeeded);
        assert_eq!(result.observed_state.as_deref(), Some("active"));
    }
}
