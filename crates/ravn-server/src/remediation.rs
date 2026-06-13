//! Remediation orchestrator — the Prepare and approval half of the PARR loop (#115).
//!
//! On a detection event the control plane matches a curated [`Template`],
//! resolves its parameters, and records a [`RemediationProposal`]. A human
//! approves (P1 is manual-approval-only; the policy engine is #116), at which
//! point the proposal is turned into a fully-resolved, **signed**
//! [`CommandEnvelope`] and enqueued for the agent to pull. The agent's reported
//! [`ActionResult`] closes the record.
//!
//! The LLM is not involved here in P1: matching is deterministic and the
//! rationale is templated. Durable Postgres audit of [`RemediationRecord`] is a
//! follow-up; P1 keeps the records in memory (mirroring the command queue).
//!
//! # Condition path validation (#151)
//!
//! Template `match.conditions` and `parameters[*].from` are dotted paths into a
//! serialized [`Event`]. Both are validated at load time against the set of paths
//! that the typed accessor [`event_field`] recognises. An unknown path causes
//! [`TemplateRegistry::load_dir`] to return an error, aborting server startup
//! with a clear message instead of silently never matching.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use ravn_core::{
    ActionResult, ApprovalRef, CommandEnvelope, Decision, Event, Payload, RemediationProposal,
    RemediationRecord, Template,
};
use uuid::Uuid;

use crate::command::CommandSigner;
use crate::policy::PolicyDecision;

// ---------------------------------------------------------------------------
// Typed field accessor (#151)
// ---------------------------------------------------------------------------

/// Walk a dotted path (e.g. `payload.unit`) through an [`Event`] and return
/// the string value at that leaf, without serialising the whole event to JSON.
///
/// Returns `None` when:
/// - the path is structurally valid but the field is absent for this payload
///   variant (e.g. `payload.unit` on a `ConfigDrift` event), or
/// - the path is not a recognised event field.
///
/// Use [`validate_event_path`] at load time to distinguish the two cases.
pub fn event_field<'e>(event: &'e Event, path: &str) -> Option<&'e str> {
    match path {
        "host" => Some(event.host.as_str()),
        "title" => Some(event.title.as_str()),
        "severity" => Some(severity_str(event.severity)),
        // payload.*
        "payload.kind" => Some(payload_kind_str(&event.payload)),
        "payload.unit" => match &event.payload {
            Payload::FailedUnit(p) => Some(p.unit.as_str()),
            Payload::Journald(p) => p.unit.as_deref(),
            _ => None,
        },
        "payload.result" => match &event.payload {
            Payload::FailedUnit(p) => Some(p.result.as_str()),
            _ => None,
        },
        "payload.message" => match &event.payload {
            Payload::Journald(p) => Some(p.message.as_str()),
            Payload::KubeWorkload(p) => p.message.as_deref(),
            Payload::KubeNode(p) => p.message.as_deref(),
            _ => None,
        },
        "payload.path" => match &event.payload {
            Payload::ConfigDrift(p) => Some(p.path.as_str()),
            _ => None,
        },
        "payload.new_hash" => match &event.payload {
            Payload::ConfigDrift(p) => Some(p.new_hash.as_str()),
            _ => None,
        },
        "payload.old_hash" => match &event.payload {
            Payload::ConfigDrift(p) => p.old_hash.as_deref(),
            _ => None,
        },
        "payload.diff" => match &event.payload {
            Payload::ConfigDrift(p) => p.diff.as_deref(),
            _ => None,
        },
        "payload.action" => match &event.payload {
            Payload::Auth(p) => Some(p.action.as_str()),
            _ => None,
        },
        "payload.user" => match &event.payload {
            Payload::Auth(p) => p.user.as_deref(),
            _ => None,
        },
        "payload.remote_addr" => match &event.payload {
            Payload::Auth(p) => p.remote_addr.as_deref(),
            _ => None,
        },
        "payload.mechanism" => match &event.payload {
            Payload::Update(p) => Some(p.mechanism.as_str()),
            _ => None,
        },
        "payload.from" => match &event.payload {
            Payload::Update(p) => p.from.as_deref(),
            _ => None,
        },
        "payload.to" => match &event.payload {
            Payload::Update(p) => p.to.as_deref(),
            _ => None,
        },
        "payload.namespace" => match &event.payload {
            Payload::KubeWorkload(p) => Some(p.namespace.as_str()),
            _ => None,
        },
        "payload.name" => match &event.payload {
            Payload::KubeWorkload(p) => Some(p.name.as_str()),
            _ => None,
        },
        "payload.object_kind" => match &event.payload {
            Payload::KubeWorkload(p) => Some(p.object_kind.as_str()),
            _ => None,
        },
        "payload.reason" => match &event.payload {
            Payload::KubeWorkload(p) => Some(p.reason.as_str()),
            _ => None,
        },
        "payload.container" => match &event.payload {
            Payload::KubeWorkload(p) => p.container.as_deref(),
            _ => None,
        },
        "payload.node" => match &event.payload {
            Payload::KubeNode(p) => Some(p.node.as_str()),
            _ => None,
        },
        "payload.condition" => match &event.payload {
            Payload::KubeNode(p) => Some(p.condition.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Returns `true` if `path` is a recognised dotted path into [`Event`].
///
/// This is distinct from `event_field` returning `Some` at runtime: a path may
/// be structurally valid yet resolve to `None` for a particular payload variant
/// (e.g. `payload.unit` is valid for `failed_unit` events but absent on
/// `config_drift`). Validation at load time checks structure only; at runtime
/// `None` means the condition simply does not match.
pub fn validate_event_path(path: &str) -> bool {
    matches!(
        path,
        "host"
            | "title"
            | "severity"
            | "payload.kind"
            | "payload.unit"
            | "payload.result"
            | "payload.message"
            | "payload.path"
            | "payload.new_hash"
            | "payload.old_hash"
            | "payload.diff"
            | "payload.action"
            | "payload.user"
            | "payload.remote_addr"
            | "payload.mechanism"
            | "payload.from"
            | "payload.to"
            | "payload.namespace"
            | "payload.name"
            | "payload.object_kind"
            | "payload.reason"
            | "payload.container"
            | "payload.node"
            | "payload.condition"
    )
}

fn severity_str(s: ravn_core::Severity) -> &'static str {
    use ravn_core::Severity::*;
    match s {
        Info => "info",
        Notice => "notice",
        Warning => "warning",
        Error => "error",
        Critical => "critical",
    }
}

fn payload_kind_str(p: &Payload) -> &'static str {
    match p {
        Payload::Journald(_) => "journald",
        Payload::FailedUnit(_) => "failed_unit",
        Payload::ConfigDrift(_) => "config_drift",
        Payload::Auth(_) => "auth",
        Payload::Update(_) => "update",
        Payload::KubeWorkload(_) => "kube_workload",
        Payload::KubeNode(_) => "kube_node",
    }
}

// ---------------------------------------------------------------------------
// Template registry
// ---------------------------------------------------------------------------

/// Curated templates loaded from a directory at startup.
#[derive(Default)]
pub struct TemplateRegistry {
    templates: Vec<Template>,
}

impl TemplateRegistry {
    /// Load and validate every `*.toml` template under `dir`. A missing
    /// directory yields an empty registry (remediation simply produces no
    /// proposals).
    ///
    /// # Startup validation (#151)
    ///
    /// Beyond the structural [`Template::validate`] already performed, this
    /// method additionally verifies that:
    ///
    /// - every `match.conditions` key is a recognised dotted [`Event`] path
    /// - every `parameters[*].from` value is a recognised dotted [`Event`] path
    ///
    /// An unrecognised path returns an `Err`, aborting server startup with a
    /// clear message rather than letting the template silently never match.
    pub fn load_dir(dir: &Path) -> anyhow::Result<Self> {
        let mut templates = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(dir = %dir.display(), "no templates directory; remediation proposals disabled");
                return Ok(Self::default());
            }
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            let template: Template = toml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("parsing template {}: {e}", path.display()))?;
            template
                .validate()
                .map_err(|e| anyhow::anyhow!("invalid template {}: {e}", path.display()))?;
            // (#151) Validate condition paths against the event schema.
            validate_template_paths(&template)
                .map_err(|e| anyhow::anyhow!("invalid template {}: {e}", path.display()))?;
            templates.push(template);
        }
        tracing::info!(count = templates.len(), dir = %dir.display(), "loaded remediation templates");
        Ok(Self { templates })
    }

    /// The first template whose `match.source` equals the event's source AND
    /// whose `match.conditions` all hold against the event.
    ///
    /// Logs at `debug` level when a template's source matches but one of its
    /// conditions does not, naming the failing path and the expected value —
    /// this makes near-miss failures observable without adding noise to the
    /// normal ingestion path (#151).
    pub fn match_event(&self, event: &Event) -> Option<&Template> {
        let source = event.source();
        for template in &self.templates {
            if template.match_.source != source {
                continue;
            }
            // Check every condition in the template.
            let mut all_match = true;
            for (path, expected) in &template.match_.conditions {
                let actual = event_field(event, path.as_str());
                if actual != Some(expected.as_str()) {
                    tracing::debug!(
                        template = %template.id,
                        condition_path = %path,
                        expected = %expected,
                        actual = ?actual,
                        event_id = %event.id,
                        host = %event.host,
                        "event matched template source but failed condition"
                    );
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return Some(template);
            }
        }
        None
    }

    /// Look a template up by id.
    pub fn get(&self, id: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.id == id)
    }
}

/// Validate that every dotted path referenced in a template's conditions and
/// parameters is recognised by [`validate_event_path`]. Returns an error
/// naming the first bad path.
fn validate_template_paths(template: &Template) -> anyhow::Result<()> {
    for path in template.match_.conditions.keys() {
        if !validate_event_path(path.as_str()) {
            anyhow::bail!(
                "template '{}': condition path '{}' is not a recognised event field; \
                 valid paths: host, title, severity, payload.kind, payload.unit, \
                 payload.result, payload.message, payload.path, payload.new_hash, \
                 payload.old_hash, payload.diff, payload.action, payload.user, \
                 payload.remote_addr, payload.mechanism, payload.from, payload.to, \
                 payload.namespace, payload.name, payload.object_kind, payload.reason, \
                 payload.container, payload.node, payload.condition",
                template.id,
                path
            );
        }
    }
    for (param_name, spec) in &template.parameters {
        if !validate_event_path(spec.from.as_str()) {
            anyhow::bail!(
                "template '{}': parameter '{}' has unrecognised from-path '{}'; \
                 valid paths: host, title, severity, payload.kind, payload.unit, \
                 payload.result, payload.message, payload.path, payload.new_hash, \
                 payload.old_hash, payload.diff, payload.action, payload.user, \
                 payload.remote_addr, payload.mechanism, payload.from, payload.to, \
                 payload.namespace, payload.name, payload.object_kind, payload.reason, \
                 payload.container, payload.node, payload.condition",
                template.id,
                param_name,
                spec.from
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parameter resolution
// ---------------------------------------------------------------------------

/// Resolve a template's declared parameters against an event using the typed
/// [`event_field`] accessor. This avoids the JSON roundtrip that the previous
/// implementation used (#151) while keeping the same dotted-path API.
pub fn resolve_params(template: &Template, event: &Event) -> anyhow::Result<BTreeMap<String, String>> {
    let mut params = BTreeMap::new();
    for (name, spec) in &template.parameters {
        let value = event_field(event, spec.from.as_str())
            .ok_or_else(|| anyhow::anyhow!("parameter {name}: no string value at '{}'", spec.from))?;
        params.insert(name.clone(), value.to_string());
    }
    Ok(params)
}

/// Build a pending proposal from a matched template and event.
pub fn build_proposal(
    template: &Template,
    event: &Event,
    params: BTreeMap<String, String>,
) -> RemediationProposal {
    let detail = params.values().cloned().collect::<Vec<_>>().join(", ");
    let rationale = format!(
        "{} on {} matched template \"{}\" ({}). Proposed: {} [{}].",
        event.title,
        event.host,
        template.title,
        template.id,
        template.title,
        if detail.is_empty() { "no parameters".into() } else { detail },
    );
    RemediationProposal {
        id: Uuid::now_v7(),
        event_id: event.id,
        agent_id: event.agent_id,
        host: event.host.clone(),
        template_id: template.id.clone(),
        template_version: template.version,
        risk_tier: template.risk_tier,
        params,
        rationale,
        created_at: Utc::now(),
    }
}

/// Turn an approved proposal into a fully-resolved, signed command for the agent.
pub fn build_command(
    proposal: &RemediationProposal,
    template: &Template,
    signer: &CommandSigner,
    approval: ApprovalRef,
    ttl_secs: i64,
) -> anyhow::Result<CommandEnvelope> {
    let steps = template
        .steps
        .iter()
        .map(|c| c.resolve(&proposal.params))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("resolving steps: {e}"))?;
    let preconditions = template
        .preconditions
        .iter()
        .map(|c| {
            Ok(ravn_core::Condition {
                check: c.check.resolve(&proposal.params)?,
                equals: c.equals.clone(),
            })
        })
        .collect::<Result<Vec<_>, ravn_core::RenderError>>()
        .map_err(|e| anyhow::anyhow!("resolving preconditions: {e}"))?;
    let verify = match &template.verify {
        Some(v) => Some(ravn_core::Verify {
            check: v.check.resolve(&proposal.params).map_err(|e| anyhow::anyhow!("resolving verify: {e}"))?,
            equals: v.equals.clone(),
            timeout_s: v.timeout_s,
        }),
        None => None,
    };
    let now = Utc::now();
    let mut env = CommandEnvelope {
        command_id: Uuid::now_v7(),
        agent_id: proposal.agent_id,
        template_id: template.id.clone(),
        template_version: template.version,
        risk_tier: template.risk_tier,
        preconditions,
        steps,
        verify,
        rollback: template.rollback,
        approval_ref: approval,
        nonce: Uuid::now_v7().to_string(),
        issued_at: now,
        expires_at: now + Duration::seconds(ttl_secs.max(1)),
        sig: None,
    };
    signer.sign(&mut env);
    Ok(env)
}

/// In-memory store of remediation records (P1; durable Postgres audit is a follow-up).
#[derive(Default)]
pub struct RemediationStore {
    records: Mutex<Vec<RemediationRecord>>,
    /// Fault signature per proposal id (#118), captured at insert time so the
    /// knowledge base can be keyed off the original event once a result lands —
    /// the closed record no longer carries the raw event payload.
    signatures: Mutex<BTreeMap<Uuid, String>>,
}

impl RemediationStore {
    /// Insert a new pending proposal, unless an identical one is already pending
    /// (dedupe recurring faults — the detection tap re-emits a failed unit every
    /// few seconds). Returns the proposal id, or `None` if deduped.
    ///
    /// `fault_signature` is the deterministic signature of the triggering event
    /// (#118), retained so the knowledge base can be updated when the result is
    /// later reported.
    pub fn insert(&self, proposal: RemediationProposal, fault_signature: String) -> Option<Uuid> {
        let mut records = self.records.lock().expect("remediation store poisoned");
        let dup = records.iter().any(|r| {
            matches!(r.decision, Decision::Pending)
                && r.proposal.template_id == proposal.template_id
                && r.proposal.agent_id == proposal.agent_id
                && r.proposal.params == proposal.params
        });
        if dup {
            return None;
        }
        let id = proposal.id;
        self.signatures
            .lock()
            .expect("remediation store poisoned")
            .insert(id, fault_signature);
        records.push(RemediationRecord {
            proposal,
            decision: Decision::Pending,
            command_id: None,
            signature: None,
            result: None,
            updated_at: Utc::now(),
        });
        Some(id)
    }

    /// Insert a record that policy auto-approved and the control plane already
    /// signed and enqueued (#116). Deduped against identical *pending* proposals
    /// just like [`Self::insert`], so a recurring fault that a human is already
    /// looking at is not also auto-fired. Returns the proposal id, or `None` if
    /// deduped.
    pub fn insert_auto_approved(
        &self,
        proposal: RemediationProposal,
        command_id: Uuid,
        signature: Option<String>,
        fault_signature: String,
    ) -> Option<Uuid> {
        let mut records = self.records.lock().expect("remediation store poisoned");
        let dup = records.iter().any(|r| {
            matches!(r.decision, Decision::Pending)
                && r.proposal.template_id == proposal.template_id
                && r.proposal.agent_id == proposal.agent_id
                && r.proposal.params == proposal.params
        });
        if dup {
            return None;
        }
        let id = proposal.id;
        // Retain the fault signature so the result later updates the KB (#118).
        self.signatures.lock().expect("remediation store poisoned").insert(id, fault_signature);
        records.push(RemediationRecord {
            proposal,
            decision: Decision::Approved { by: ApprovalRef::PolicyAuto },
            command_id: Some(command_id),
            signature,
            result: None,
            updated_at: Utc::now(),
        });
        Some(id)
    }

    /// All records, newest first.
    pub fn list(&self) -> Vec<RemediationRecord> {
        let mut v = self.records.lock().expect("remediation store poisoned").clone();
        v.reverse();
        v
    }

    /// The proposal for a pending record, if it exists and is still pending.
    pub fn pending_proposal(&self, id: Uuid) -> Option<RemediationProposal> {
        let records = self.records.lock().expect("remediation store poisoned");
        records
            .iter()
            .find(|r| r.proposal.id == id && matches!(r.decision, Decision::Pending))
            .map(|r| r.proposal.clone())
    }

    /// Record approval and the issued signed command.
    pub fn approve(&self, id: Uuid, by: ApprovalRef, command_id: Uuid, signature: Option<String>) -> bool {
        self.update(id, |r| {
            r.decision = Decision::Approved { by: by.clone() };
            r.command_id = Some(command_id);
            r.signature = signature.clone();
        })
    }

    /// Record a rejection.
    pub fn reject(&self, id: Uuid, by: String, reason: Option<String>) -> bool {
        self.update(id, |r| {
            r.decision = Decision::Rejected { by: by.clone(), at: Utc::now(), reason: reason.clone() };
        })
    }

    /// Attach the agent-reported result to the record carrying `command_id`,
    /// returning the now-closed record together with its fault signature (so
    /// callers can reflect it into the knowledge base, #118). `None` if no record
    /// matches the command.
    pub fn record_result(&self, result: ActionResult) -> Option<(RemediationRecord, String)> {
        let mut records = self.records.lock().expect("remediation store poisoned");
        let r = records.iter_mut().find(|r| r.command_id == Some(result.command_id))?;
        r.result = Some(result);
        r.updated_at = Utc::now();
        let record = r.clone();
        let signature = self
            .signatures
            .lock()
            .expect("remediation store poisoned")
            .get(&record.proposal.id)
            .cloned()
            .unwrap_or_default();
        Some((record, signature))
    }

    fn update(&self, id: Uuid, f: impl FnOnce(&mut RemediationRecord)) -> bool {
        let mut records = self.records.lock().expect("remediation store poisoned");
        if let Some(r) = records.iter_mut().find(|r| r.proposal.id == id) {
            f(r);
            r.updated_at = Utc::now();
            return true;
        }
        false
    }
}

/// Prepare hook, called best-effort after a message is persisted (#115). Matches
/// a template and resolves parameters, then asks the policy engine (#116) what
/// to do: `auto` signs + enqueues the command immediately and records it as
/// approved-by-[`ApprovalRef::PolicyAuto`]; `approve` records a pending proposal
/// for a human (the default); `forbid` produces nothing. Never blocks or errors
/// the ingestion path.
///
/// Knowledge base (#118): on a *matched* fault the proposal's rationale is biased
/// with a deterministic recall note ("last N× this fired, template X succeeded");
/// on an *unmatched* fault a `gap` entry is written so operators can see which
/// faults still need a template.
pub fn prepare(state: &crate::state::AppState, message: &ravn_core::Message) {
    let event = &message.event;
    let signature = crate::knowledge::fault_signature(event);
    let Some(template) = state.templates.match_event(event) else {
        // No template for this fault — record a gap so the catalog can grow.
        state.knowledge.record_gap(event);
        return;
    };
    let params = match resolve_params(template, event) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(%e, template = %template.id, "could not resolve template parameters");
            return;
        }
    };
    let template_id = template.id.clone();
    let mut proposal = build_proposal(template, event, params);
    // Deterministic recall: surface past resolutions of this exact fault (#118).
    if let Some(note) = state.knowledge.recall(&signature) {
        proposal.rationale = format!("{} {}", proposal.rationale, note);
    }

    match state.policy.evaluate(&event.host, template.risk_tier, &template_id, Utc::now()) {
        PolicyDecision::Forbid => {
            tracing::info!(template = %template_id, host = %event.host, "remediation forbidden by policy");
        }
        PolicyDecision::Approve => {
            if let Some(id) = state.remediations.insert(proposal, signature) {
                tracing::info!(proposal = %id, template = %template_id, host = %event.host, "remediation proposed (awaiting approval)");
            }
        }
        PolicyDecision::Auto => {
            auto_execute(state, template, proposal, &template_id, &event.host, signature)
        }
    }
}

/// Sign, enqueue, and record a policy-auto-approved remediation. Best-effort: a
/// build failure is logged and dropped (it must never disturb ingestion).
fn auto_execute(
    state: &crate::state::AppState,
    template: &Template,
    proposal: RemediationProposal,
    template_id: &str,
    host: &str,
    fault_signature: String,
) {
    let envelope = match build_command(
        &proposal,
        template,
        &state.command_signer,
        ApprovalRef::PolicyAuto,
        state.command_ttl_secs,
    ) {
        Ok(env) => env,
        Err(e) => {
            tracing::warn!(%e, template = %template_id, host, "could not build auto-remediation command");
            return;
        }
    };
    let command_id = envelope.command_id;
    let signature = envelope.sig.clone();
    if let Some(id) =
        state.remediations.insert_auto_approved(proposal, command_id, signature, fault_signature)
    {
        state.command_queue.enqueue(envelope);
        tracing::info!(proposal = %id, %command_id, template = %template_id, host, "remediation auto-executed by policy");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{ActionStatus, AgentId, Capability, FailedUnitPayload, Payload, Severity, Source};
    use ravn_crypto::{verify_envelope, verifying_key_from_b64};

    fn failed_unit_event(unit: &str) -> Event {
        let now = Utc::now();
        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::now_v7()),
            host: "host-1".into(),
            severity: Severity::Error,
            title: format!("{unit} failed"),
            category_hints: vec![],
            payload: Payload::FailedUnit(FailedUnitPayload {
                unit: unit.into(),
                result: "exit-code".into(),
                ..Default::default()
            }),
        }
    }

    fn registry() -> TemplateRegistry {
        TemplateRegistry::load_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates")))
            .expect("load templates")
    }

    #[test]
    fn loads_and_matches_the_failed_unit_template() {
        let reg = registry();
        let tpl = reg.match_event(&failed_unit_event("nginx.service")).expect("match");
        assert_eq!(tpl.id, "failed-unit-restart");
        assert_eq!(tpl.match_.source, Source::FailedUnit);
    }

    #[test]
    fn resolves_unit_param_from_payload() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let params = resolve_params(tpl, &event).unwrap();
        assert_eq!(params.get("unit").map(String::as_str), Some("nginx.service"));
    }

    #[test]
    fn build_command_resolves_placeholders_and_signs() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let params = resolve_params(tpl, &event).unwrap();
        let proposal = build_proposal(tpl, &event, params);
        let signer = CommandSigner::load_or_generate(None).unwrap();

        let env = build_command(&proposal, tpl, &signer, ApprovalRef::PolicyAuto, 300).unwrap();
        // Steps resolved — no placeholders remain.
        assert_eq!(
            env.steps,
            vec![
                Capability::ResetFailed { unit: "nginx.service".into() },
                Capability::RestartUnit { unit: "nginx.service".into() },
            ]
        );
        // And it verifies against the signer's public key.
        let pk = verifying_key_from_b64(signer.pubkey_b64()).unwrap();
        verify_envelope(&pk, &env, Utc::now()).unwrap();
    }

    #[test]
    fn insert_auto_approved_records_policy_auto_and_command() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let proposal = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let store = RemediationStore::default();

        let command_id = Uuid::now_v7();
        let id = store
            .insert_auto_approved(proposal, command_id, Some("sig".into()), "sig:fault".into())
            .expect("inserted");
        // It is already approved (not pending) and carries the command.
        assert!(store.pending_proposal(id).is_none(), "auto-approved is not pending");
        let rec = store.list().into_iter().find(|r| r.proposal.id == id).unwrap();
        assert!(matches!(rec.decision, Decision::Approved { by: ApprovalRef::PolicyAuto }));
        assert_eq!(rec.command_id, Some(command_id));
        assert_eq!(rec.signature.as_deref(), Some("sig"));
    }

    #[test]
    fn insert_auto_approved_dedupes_against_pending() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let store = RemediationStore::default();

        // A human is already looking at an identical pending proposal …
        let pending = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        assert!(store.insert(pending, "sig:fault".into()).is_some());
        // … so an auto attempt for the same fault is deduped (not double-fired).
        let auto = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        assert!(store.insert_auto_approved(auto, Uuid::now_v7(), None, "sig:fault".into()).is_none());
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn store_lifecycle_insert_approve_result() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let proposal = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let store = RemediationStore::default();

        let id = store.insert(proposal.clone(), "sig".into()).expect("inserted");
        assert!(store.pending_proposal(id).is_some());

        let command_id = Uuid::now_v7();
        assert!(store.approve(
            id,
            ApprovalRef::Human { user: "olaf".into(), approved_at: Utc::now() },
            command_id,
            Some("sig".into()),
        ));
        assert!(store.pending_proposal(id).is_none(), "approved record is no longer pending");

        assert!(store
            .record_result(ActionResult {
                command_id,
                status: ActionStatus::Succeeded,
                detail: None,
                observed_state: Some("active".into()),
                finished_at: Utc::now(),
            })
            .is_some());
        let rec = store.list().into_iter().find(|r| r.proposal.id == id).unwrap();
        assert!(matches!(rec.decision, Decision::Approved { .. }));
        assert_eq!(rec.result.unwrap().status, ActionStatus::Succeeded);
    }

    #[test]
    fn store_dedupes_identical_pending_proposals() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let store = RemediationStore::default();

        let p1 = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let p2 = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        assert!(store.insert(p1, "sig".into()).is_some());
        assert!(store.insert(p2, "sig".into()).is_none(), "identical pending proposal is deduped");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn reject_marks_record_rejected() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let store = RemediationStore::default();
        let proposal = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let id = store.insert(proposal, "sig".into()).unwrap();
        assert!(store.reject(id, "olaf".into(), Some("not now".into())));
        let rec = store.list().into_iter().next().unwrap();
        assert!(matches!(rec.decision, Decision::Rejected { .. }));
    }

    // ------------------------------------------------------------------
    // Issue #151: typed path resolution + startup validation
    // ------------------------------------------------------------------

    /// A template with an unrecognised condition path must fail load_dir, not
    /// silently load and never match.
    #[test]
    fn load_dir_rejects_bad_condition_path() {
        let dir = tempfile::tempdir().unwrap();
        let bad_toml = r#"
            id = "bad-cond"
            version = 1
            title = "bad"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            conditions = { active_state = "failed" }
            [[steps]]
            capability = "restart_unit"
            unit = "x.service"
        "#;
        std::fs::write(dir.path().join("bad.toml"), bad_toml).unwrap();
        let result = TemplateRegistry::load_dir(dir.path()).map(|_| ());
        assert!(result.is_err(), "expected load_dir to fail with bad condition path");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("active_state"),
            "error message should name the bad path, got: {msg}"
        );
        assert!(
            msg.contains("bad.toml") || msg.contains("bad-cond"),
            "error should name the template or file, got: {msg}"
        );
    }

    /// A template with an unrecognised parameter `from` path must fail load_dir.
    #[test]
    fn load_dir_rejects_bad_parameter_from_path() {
        let dir = tempfile::tempdir().unwrap();
        let bad_toml = r#"
            id = "bad-param"
            version = 1
            title = "bad"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            [parameters]
            unit = { type = "string", from = "payload.typo_unit" }
            [[steps]]
            capability = "restart_unit"
            unit = "{{unit}}"
        "#;
        std::fs::write(dir.path().join("bad.toml"), bad_toml).unwrap();
        let result = TemplateRegistry::load_dir(dir.path()).map(|_| ());
        assert!(result.is_err(), "expected load_dir to fail with bad parameter from-path");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("payload.typo_unit"),
            "error message should name the bad path, got: {msg}"
        );
    }

    /// `match_event` must return `None` when the source matches but a condition
    /// fails (i.e. the field value does not equal the expected value).
    #[test]
    fn match_event_skips_template_when_condition_fails() {
        let dir = tempfile::tempdir().unwrap();
        // Template requires payload.result = "timeout", but the event has "exit-code".
        let toml_src = r#"
            id = "timeout-only"
            version = 1
            title = "timeout only"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            conditions = { "payload.result" = "timeout" }
            [parameters]
            unit = { type = "string", from = "payload.unit" }
            [[steps]]
            capability = "restart_unit"
            unit = "{{unit}}"
        "#;
        std::fs::write(dir.path().join("t.toml"), toml_src).unwrap();
        let reg = TemplateRegistry::load_dir(dir.path()).expect("valid template");
        // Event has result = "exit-code", not "timeout".
        let event = failed_unit_event("nginx.service");
        assert!(
            reg.match_event(&event).is_none(),
            "should not match when condition value differs"
        );
    }

    /// `match_event` returns the template when source AND all conditions match.
    #[test]
    fn match_event_returns_template_when_all_conditions_pass() {
        let dir = tempfile::tempdir().unwrap();
        let toml_src = r#"
            id = "exit-code-only"
            version = 1
            title = "exit-code only"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            conditions = { "payload.result" = "exit-code" }
            [parameters]
            unit = { type = "string", from = "payload.unit" }
            [[steps]]
            capability = "restart_unit"
            unit = "{{unit}}"
        "#;
        std::fs::write(dir.path().join("t.toml"), toml_src).unwrap();
        let reg = TemplateRegistry::load_dir(dir.path()).expect("valid template");
        let event = failed_unit_event("nginx.service"); // result = "exit-code"
        let tpl = reg.match_event(&event).expect("should match");
        assert_eq!(tpl.id, "exit-code-only");
    }

    /// The typed accessor must return the right string for common paths.
    #[test]
    fn event_field_returns_typed_values() {
        let event = failed_unit_event("nginx.service");
        assert_eq!(event_field(&event, "host"), Some("host-1"));
        assert_eq!(event_field(&event, "severity"), Some("error"));
        assert_eq!(event_field(&event, "payload.kind"), Some("failed_unit"));
        assert_eq!(event_field(&event, "payload.unit"), Some("nginx.service"));
        assert_eq!(event_field(&event, "payload.result"), Some("exit-code"));
        // Wrong payload variant → None, not a panic.
        assert_eq!(event_field(&event, "payload.path"), None);
        // Completely unknown path → None.
        assert_eq!(event_field(&event, "nonexistent.field"), None);
    }

    /// `validate_event_path` accepts recognised paths and rejects unknown ones.
    #[test]
    fn validate_event_path_accepts_known_and_rejects_unknown() {
        assert!(validate_event_path("host"));
        assert!(validate_event_path("payload.unit"));
        assert!(validate_event_path("payload.condition"));
        assert!(!validate_event_path("active_state"));
        assert!(!validate_event_path("payload.typo_unit"));
        assert!(!validate_event_path(""));
    }

    /// `resolve_params` uses the typed accessor and resolves correctly.
    #[test]
    fn resolve_params_uses_typed_accessor() {
        let dir = tempfile::tempdir().unwrap();
        let toml_src = r#"
            id = "typed-params"
            version = 1
            title = "typed"
            risk_tier = "safe"
            [match]
            source = "failed_unit"
            [parameters]
            unit = { type = "string", from = "payload.unit" }
            host = { type = "string", from = "host" }
            [[steps]]
            capability = "restart_unit"
            unit = "{{unit}}"
        "#;
        std::fs::write(dir.path().join("t.toml"), toml_src).unwrap();
        let reg = TemplateRegistry::load_dir(dir.path()).expect("valid template");
        let event = failed_unit_event("sshd.service");
        let tpl = reg.match_event(&event).expect("match");
        let params = resolve_params(tpl, &event).expect("resolve");
        assert_eq!(params["unit"], "sshd.service");
        assert_eq!(params["host"], "host-1");
    }
}
