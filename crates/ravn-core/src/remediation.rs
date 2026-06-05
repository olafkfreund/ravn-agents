//! Remediation schema (#112): the action-side mirror of detection.
//!
//! See `plans/2026-06-05-ravn-self-healing-remediation-design.md`. Doctrine: the
//! LLM never appears in these types. They describe *pre-authored, deterministic*
//! remediations that a human or a signed policy approves; the actuator executes
//! only the typed [`Capability`] variants. A wrong model produces a bad
//! [`RemediationProposal`] — rejected upstream — never a bad action.
//!
//! Crypto deliberately lives elsewhere (#114): this module carries the
//! [`CommandEnvelope`] type and a deterministic [`CommandEnvelope::signing_payload`]
//! (the exact bytes to sign/verify), but no signing key handling, so the shared
//! core crate stays dependency-light.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::{AgentId, Source};

/// How dangerous a remediation is, ordered least-to-most. The ordering lets the
/// policy engine (#116) threshold on it (`Safe < Dangerous`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    /// Idempotent, no data loss (e.g. `reset_failed`, `restart_unit`).
    Safe,
    /// Service-affecting but reversible (e.g. `nix_rollback`, `kill_process`).
    Guarded,
    /// Potential data loss or wide blast radius. Never auto-executes.
    Dangerous,
}

/// A single typed, whitelisted operation. The actuator (#113) implements exactly
/// these and nothing else — there is no arbitrary-shell variant by design.
///
/// On the wire this is internally tagged by `capability`, e.g.
/// `{ "capability": "restart_unit", "unit": "nginx.service" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "capability", rename_all = "snake_case")]
pub enum Capability {
    /// Clear a unit's failed state.
    ResetFailed { unit: String },
    /// Restart a systemd unit.
    RestartUnit { unit: String },
    /// Read-only: report a unit's active state (used in checks).
    UnitState { unit: String },
}

impl Capability {
    /// Whether this capability only observes (never mutates) host state. Used to
    /// constrain which capabilities may appear in pre/post-condition checks.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Capability::UnitState { .. })
    }

    /// Resolve `{{param}}` placeholders in this capability's string fields against
    /// `values`, returning a concrete capability with no placeholders left.
    pub fn resolve(&self, values: &BTreeMap<String, String>) -> Result<Capability, RenderError> {
        Ok(match self {
            Capability::ResetFailed { unit } => {
                Capability::ResetFailed { unit: resolve_str(unit, values)? }
            }
            Capability::RestartUnit { unit } => {
                Capability::RestartUnit { unit: resolve_str(unit, values)? }
            }
            Capability::UnitState { unit } => {
                Capability::UnitState { unit: resolve_str(unit, values)? }
            }
        })
    }
}

/// A read-only check plus the value it must equal — used for preconditions
/// (verified before acting) and verification (verified after).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Condition {
    /// The read-only capability to evaluate.
    pub check: Capability,
    /// The observed value the check must equal for the condition to hold.
    pub equals: String,
}

/// A post-condition with a bounded wait for the desired state to appear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Verify {
    /// The read-only capability to evaluate after acting.
    pub check: Capability,
    /// The observed value that means the remediation succeeded.
    pub equals: String,
    /// How long to wait for the desired state, in seconds.
    #[serde(default = "default_verify_timeout")]
    pub timeout_s: u64,
}

fn default_verify_timeout() -> u64 {
    30
}

/// What to do if verification fails. (Capability-based rollback is added in P3.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rollback {
    /// No rollback (the remediation is idempotent/safe).
    #[default]
    None,
    /// Roll the host back to its previous NixOS generation — the universal net.
    NixGeneration,
}

/// The kind of a declared template parameter. Only strings exist in P1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    String,
}

/// A declared template parameter and where its value comes from on the
/// triggering event (e.g. `payload.unit`). Resolution lives in the orchestrator
/// (#115); this is just the declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParameterSpec {
    #[serde(rename = "type")]
    pub ty: ParamType,
    /// Dotted path into the triggering event, e.g. `payload.unit`.
    pub from: String,
}

/// The fault signature a template matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateMatch {
    /// The detection source this template responds to.
    pub source: Source,
    /// Extra equality conditions on the event (e.g. `active_state = "failed"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub conditions: BTreeMap<String, String>,
}

/// A pre-authored, deterministic remediation. Authored in git as TOML, reviewed
/// by humans; it composes [`Capability`] steps and never contains shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Template {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub risk_tier: RiskTier,
    /// The fault this template matches. `match` is a Rust keyword, hence the rename.
    #[serde(rename = "match")]
    pub match_: TemplateMatch,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, ParameterSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<Condition>,
    /// The ordered capabilities to run.
    pub steps: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<Verify>,
    #[serde(default)]
    pub rollback: Rollback,
}

impl Template {
    /// Structurally validate the template: every `{{param}}` placeholder used in
    /// preconditions, steps, or verify must be a declared parameter, and every
    /// check capability must be read-only.
    pub fn validate(&self) -> Result<(), TemplateError> {
        let declared: Vec<&str> = self.parameters.keys().map(String::as_str).collect();

        let check_placeholders = |cap: &Capability| -> Result<(), TemplateError> {
            for token in placeholders_in_capability(cap) {
                if !declared.contains(&token.as_str()) {
                    return Err(TemplateError::UndeclaredParameter(token));
                }
            }
            Ok(())
        };

        for cond in &self.preconditions {
            if !cond.check.is_read_only() {
                return Err(TemplateError::NonReadOnlyCheck);
            }
            check_placeholders(&cond.check)?;
        }
        for step in &self.steps {
            check_placeholders(step)?;
        }
        if let Some(verify) = &self.verify {
            if !verify.check.is_read_only() {
                return Err(TemplateError::NonReadOnlyCheck);
            }
            check_placeholders(&verify.check)?;
        }
        if self.steps.is_empty() {
            return Err(TemplateError::NoSteps);
        }
        Ok(())
    }
}

/// Why a [`Template`] failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// A `{{name}}` placeholder referenced a parameter the template didn't declare.
    UndeclaredParameter(String),
    /// A precondition or verify used a capability that mutates host state.
    NonReadOnlyCheck,
    /// The template declared no steps to run.
    NoSteps,
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::UndeclaredParameter(p) => {
                write!(f, "template uses undeclared parameter {{{{{p}}}}}")
            }
            TemplateError::NonReadOnlyCheck => {
                write!(f, "preconditions and verify must use read-only check capabilities")
            }
            TemplateError::NoSteps => write!(f, "template declares no steps"),
        }
    }
}

impl std::error::Error for TemplateError {}

/// Failure resolving `{{param}}` placeholders against supplied values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// A placeholder had no supplied value.
    MissingValue(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::MissingValue(p) => write!(f, "no value supplied for parameter {{{{{p}}}}}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Replace every `{{name}}` in `raw` with `values[name]`, erroring on any unknown
/// placeholder. Whitespace inside the braces is tolerated (`{{ name }}`).
pub fn resolve_str(raw: &str, values: &BTreeMap<String, String>) -> Result<String, RenderError> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // No closing braces — treat the rest as literal.
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = after[..end].trim();
        let value = values.get(name).ok_or_else(|| RenderError::MissingValue(name.to_string()))?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Collect the placeholder names referenced by a capability's string fields.
fn placeholders_in_capability(cap: &Capability) -> Vec<String> {
    let field = match cap {
        Capability::ResetFailed { unit }
        | Capability::RestartUnit { unit }
        | Capability::UnitState { unit } => unit,
    };
    placeholders_in_str(field)
}

fn placeholders_in_str(raw: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        names.push(after[..end].trim().to_string());
        rest = &after[end + 2..];
    }
    names
}

/// Who or what authorised an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalRef {
    /// Approved automatically by a signed policy (#116, P2).
    PolicyAuto,
    /// Approved by a named operator (the OIDC identity) at a time.
    Human { user: String, approved_at: DateTime<Utc> },
}

/// A signed, ready-to-execute command pulled by the agent (#114 fills `sig`).
///
/// `steps` are fully resolved — no `{{placeholders}}` remain. The signed bytes
/// are exactly [`CommandEnvelope::signing_payload`], which excludes `sig` itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandEnvelope {
    pub command_id: Uuid,
    pub agent_id: AgentId,
    pub template_id: String,
    pub template_version: u32,
    pub risk_tier: RiskTier,
    /// Ordered, fully-resolved capabilities to execute.
    pub steps: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<Verify>,
    pub approval_ref: ApprovalRef,
    /// Random per-command nonce; with `expires_at` this defeats replay.
    pub nonce: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Base64 Ed25519 signature over [`Self::signing_payload`]. `None` until the
    /// control plane signs it (#114); the agent rejects an envelope whose `sig`
    /// is absent or invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

impl CommandEnvelope {
    /// The exact, deterministic bytes that get signed and verified — every field
    /// except `sig`. Stable across runs: struct fields serialize in declaration
    /// order and the only maps involved are sorted `BTreeMap`s.
    pub fn signing_payload(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct Signable<'a> {
            command_id: &'a Uuid,
            agent_id: &'a AgentId,
            template_id: &'a str,
            template_version: u32,
            risk_tier: &'a RiskTier,
            steps: &'a [Capability],
            verify: &'a Option<Verify>,
            approval_ref: &'a ApprovalRef,
            nonce: &'a str,
            issued_at: &'a DateTime<Utc>,
            expires_at: &'a DateTime<Utc>,
        }
        let signable = Signable {
            command_id: &self.command_id,
            agent_id: &self.agent_id,
            template_id: &self.template_id,
            template_version: self.template_version,
            risk_tier: &self.risk_tier,
            steps: &self.steps,
            verify: &self.verify,
            approval_ref: &self.approval_ref,
            nonce: &self.nonce,
            issued_at: &self.issued_at,
            expires_at: &self.expires_at,
        };
        serde_json::to_vec(&signable).expect("CommandEnvelope signable fields always serialize")
    }
}

/// Outcome of executing a [`CommandEnvelope`], reported by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    /// All steps ran and verification (if any) passed.
    Succeeded,
    /// A step or verification failed.
    Failed,
    /// The agent or actuator refused the command (bad signature, expired, etc.).
    Rejected,
}

/// The agent's report on a command's execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionResult {
    pub command_id: Uuid,
    pub status: ActionStatus,
    /// Human-readable detail (error text, or a note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The observed post-state (e.g. the unit's active state), when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_state: Option<String>,
    pub finished_at: DateTime<Utc>,
}

/// A proposed remediation awaiting a decision (the output of Prepare, #115).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RemediationProposal {
    pub id: Uuid,
    /// The triggering detection event.
    pub event_id: Uuid,
    pub agent_id: AgentId,
    pub host: String,
    pub template_id: String,
    pub template_version: u32,
    pub risk_tier: RiskTier,
    /// Resolved parameter values for the chosen template.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Plain-language reason this remediation was proposed (templated in P1; LLM
    /// phrasing is a later enhancement).
    pub rationale: String,
    pub created_at: DateTime<Utc>,
}

/// The decision taken on a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Awaiting a human (or, later, a policy) decision.
    Pending,
    /// Approved for execution.
    Approved { by: ApprovalRef },
    /// Rejected; not executed.
    Rejected {
        by: String,
        at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// The immutable audit trail of a remediation's whole lifecycle (#115).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RemediationRecord {
    pub proposal: RemediationProposal,
    pub decision: Decision,
    /// The signed command issued on approval, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<Uuid>,
    /// The base64 signature of the issued command, recorded for audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// The execution result, once reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ActionResult>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAILED_UNIT_RESTART: &str = r#"
        id = "failed-unit-restart"
        version = 3
        title = "Restart a failed systemd unit"
        risk_tier = "safe"

        [match]
        source = "failed_unit"
        conditions = { active_state = "failed" }

        [parameters]
        unit = { type = "string", from = "payload.unit" }

        [[preconditions]]
        equals = "failed"
        check = { capability = "unit_state", unit = "{{unit}}" }

        [[steps]]
        capability = "reset_failed"
        unit = "{{unit}}"

        [[steps]]
        capability = "restart_unit"
        unit = "{{unit}}"

        [verify]
        equals = "active"
        timeout_s = 30
        check = { capability = "unit_state", unit = "{{unit}}" }

        rollback = "none"
    "#;

    fn values(unit: &str) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("unit".to_string(), unit.to_string());
        m
    }

    #[test]
    fn example_template_parses_and_validates() {
        let tpl: Template = toml::from_str(FAILED_UNIT_RESTART).expect("parse");
        assert_eq!(tpl.id, "failed-unit-restart");
        assert_eq!(tpl.version, 3);
        assert_eq!(tpl.risk_tier, RiskTier::Safe);
        assert_eq!(tpl.match_.source, Source::FailedUnit);
        assert_eq!(tpl.match_.conditions.get("active_state").map(String::as_str), Some("failed"));
        assert_eq!(tpl.steps.len(), 2);
        assert_eq!(tpl.verify.as_ref().unwrap().timeout_s, 30);
        assert_eq!(tpl.rollback, Rollback::None);
        tpl.validate().expect("valid");
    }

    #[test]
    fn verify_timeout_defaults_to_30() {
        let toml_src = r#"
            id = "t"
            version = 1
            title = "t"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            [[steps]]
            capability = "restart_unit"
            unit = "x.service"
            [verify]
            equals = "active"
            check = { capability = "unit_state", unit = "x.service" }
        "#;
        let tpl: Template = toml::from_str(toml_src).unwrap();
        assert_eq!(tpl.verify.unwrap().timeout_s, 30);
    }

    #[test]
    fn validate_rejects_undeclared_parameter() {
        let toml_src = r#"
            id = "t"
            version = 1
            title = "t"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            [[steps]]
            capability = "restart_unit"
            unit = "{{missing}}"
        "#;
        let tpl: Template = toml::from_str(toml_src).unwrap();
        assert_eq!(
            tpl.validate().unwrap_err(),
            TemplateError::UndeclaredParameter("missing".to_string())
        );
    }

    #[test]
    fn validate_rejects_mutating_check() {
        let mut tpl: Template = toml::from_str(FAILED_UNIT_RESTART).unwrap();
        // Swap the verify check for a mutating capability.
        tpl.verify = Some(Verify {
            check: Capability::RestartUnit { unit: "x".into() },
            equals: "active".into(),
            timeout_s: 30,
        });
        assert_eq!(tpl.validate().unwrap_err(), TemplateError::NonReadOnlyCheck);
    }

    #[test]
    fn capability_resolves_placeholders() {
        let cap = Capability::RestartUnit { unit: "{{unit}}".into() };
        let resolved = cap.resolve(&values("nginx.service")).unwrap();
        assert_eq!(resolved, Capability::RestartUnit { unit: "nginx.service".into() });
    }

    #[test]
    fn resolve_str_errors_on_missing_value() {
        assert_eq!(
            resolve_str("{{nope}}", &values("x")).unwrap_err(),
            RenderError::MissingValue("nope".to_string())
        );
    }

    #[test]
    fn capability_is_internally_tagged() {
        let cap = Capability::RestartUnit { unit: "nginx.service".into() };
        let json = serde_json::to_value(&cap).unwrap();
        assert_eq!(json["capability"], "restart_unit");
        assert_eq!(json["unit"], "nginx.service");
        let back: Capability = serde_json::from_value(json).unwrap();
        assert_eq!(back, cap);
    }

    #[test]
    fn signing_payload_excludes_sig_and_is_stable() {
        let now = Utc::now();
        let mut env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "failed-unit-restart".into(),
            template_version: 3,
            risk_tier: RiskTier::Safe,
            steps: vec![Capability::RestartUnit { unit: "nginx.service".into() }],
            verify: None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "abc123".into(),
            issued_at: now,
            expires_at: now,
            sig: None,
        };
        let unsigned = env.signing_payload();
        env.sig = Some("signature-bytes".into());
        let signed = env.signing_payload();
        assert_eq!(unsigned, signed, "sig must not affect the signing payload");
        // Deterministic across calls.
        assert_eq!(env.signing_payload(), env.signing_payload());
    }

    #[test]
    fn envelope_round_trips() {
        let now = Utc::now();
        let env = CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "t".into(),
            template_version: 1,
            risk_tier: RiskTier::Guarded,
            steps: vec![
                Capability::ResetFailed { unit: "a.service".into() },
                Capability::RestartUnit { unit: "a.service".into() },
            ],
            verify: Some(Verify {
                check: Capability::UnitState { unit: "a.service".into() },
                equals: "active".into(),
                timeout_s: 15,
            }),
            approval_ref: ApprovalRef::Human { user: "olaf".into(), approved_at: now },
            nonce: "n".into(),
            issued_at: now,
            expires_at: now,
            sig: Some("sig".into()),
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: CommandEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn schemas_generate_without_panicking() {
        let _ = schemars::schema_for!(Template);
        let _ = schemars::schema_for!(CommandEnvelope);
        let _ = schemars::schema_for!(RemediationRecord);
    }
}
