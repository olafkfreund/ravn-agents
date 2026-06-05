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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use ravn_core::{
    ActionResult, ApprovalRef, CommandEnvelope, Decision, Event, RemediationProposal,
    RemediationRecord, Template,
};
use uuid::Uuid;

use crate::command::CommandSigner;

/// Curated templates loaded from a directory at startup.
#[derive(Default)]
pub struct TemplateRegistry {
    templates: Vec<Template>,
}

impl TemplateRegistry {
    /// Load and validate every `*.toml` template under `dir`. A missing directory
    /// yields an empty registry (remediation simply produces no proposals).
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
            templates.push(template);
        }
        tracing::info!(count = templates.len(), dir = %dir.display(), "loaded remediation templates");
        Ok(Self { templates })
    }

    /// The first template whose match source equals the event's source.
    /// (Richer `conditions` matching is part of the policy work, #116.)
    pub fn match_event(&self, event: &Event) -> Option<&Template> {
        let source = event.source();
        self.templates.iter().find(|t| t.match_.source == source)
    }

    /// Look a template up by id.
    pub fn get(&self, id: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.id == id)
    }
}

/// Resolve a template's declared parameters against an event by navigating the
/// dotted `from` path (e.g. `payload.unit`) through the event's JSON form.
pub fn resolve_params(template: &Template, event: &Event) -> anyhow::Result<BTreeMap<String, String>> {
    let event_json = serde_json::to_value(event)?;
    let mut params = BTreeMap::new();
    for (name, spec) in &template.parameters {
        let mut cur = &event_json;
        for segment in spec.from.split('.') {
            cur = cur
                .get(segment)
                .ok_or_else(|| anyhow::anyhow!("parameter {name}: no field at {}", spec.from))?;
        }
        let value = cur
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("parameter {name}: {} is not a string", spec.from))?;
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
}

impl RemediationStore {
    /// Insert a new pending proposal, unless an identical one is already pending
    /// (dedupe recurring faults — the detection tap re-emits a failed unit every
    /// few seconds). Returns the proposal id, or `None` if deduped.
    pub fn insert(&self, proposal: RemediationProposal) -> Option<Uuid> {
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

    /// Attach the agent-reported result to the record carrying `command_id`.
    pub fn record_result(&self, result: ActionResult) -> bool {
        let mut records = self.records.lock().expect("remediation store poisoned");
        if let Some(r) = records.iter_mut().find(|r| r.command_id == Some(result.command_id)) {
            r.result = Some(result);
            r.updated_at = Utc::now();
            return true;
        }
        false
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
/// a template, resolves parameters, and records a pending proposal. Never blocks
/// or errors the ingestion path.
pub fn prepare(state: &crate::state::AppState, message: &ravn_core::Message) {
    let event = &message.event;
    let Some(template) = state.templates.match_event(event) else { return };
    let params = match resolve_params(template, event) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(%e, template = %template.id, "could not resolve template parameters");
            return;
        }
    };
    let template_id = template.id.clone();
    let proposal = build_proposal(template, event, params);
    if let Some(id) = state.remediations.insert(proposal) {
        tracing::info!(proposal = %id, template = %template_id, host = %event.host, "remediation proposed");
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
    fn store_lifecycle_insert_approve_result() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let proposal = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let store = RemediationStore::default();

        let id = store.insert(proposal.clone()).expect("inserted");
        assert!(store.pending_proposal(id).is_some());

        let command_id = Uuid::now_v7();
        assert!(store.approve(
            id,
            ApprovalRef::Human { user: "olaf".into(), approved_at: Utc::now() },
            command_id,
            Some("sig".into()),
        ));
        assert!(store.pending_proposal(id).is_none(), "approved record is no longer pending");

        assert!(store.record_result(ActionResult {
            command_id,
            status: ActionStatus::Succeeded,
            detail: None,
            observed_state: Some("active".into()),
            finished_at: Utc::now(),
        }));
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
        assert!(store.insert(p1).is_some());
        assert!(store.insert(p2).is_none(), "identical pending proposal is deduped");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn reject_marks_record_rejected() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let store = RemediationStore::default();
        let proposal = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let id = store.insert(proposal).unwrap();
        assert!(store.reject(id, "olaf".into(), Some("not now".into())));
        let rec = store.list().into_iter().next().unwrap();
        assert!(matches!(rec.decision, Decision::Rejected { .. }));
    }
}
