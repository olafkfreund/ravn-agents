//! Remediation orchestrator — the Prepare and approval half of the PARR loop (#115).
//!
//! On a detection event the control plane matches a curated [`Template`],
//! resolves its parameters, and records a [`RemediationProposal`]. A human
//! approves (P1 is manual-approval-only; the policy engine is #116), at which
//! point the proposal is turned into a fully-resolved, **signed**
//! [`CommandEnvelope`] and enqueued for the agent to pull. The agent's reported
//! [`ActionResult`] closes the record.
//!
//! The LLM is not involved here: matching is deterministic and the rationale is
//! templated. This module implements #143: **durable Postgres audit** replaces
//! the former in-memory store. `RemediationStore` now wraps a `PgPool`; every
//! state transition is written to `remediation_records` at the moment it
//! occurs. An in-memory dedup cache guards the hot ingest path against repeat
//! inserts without a DB round-trip.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use ravn_core::{
    ActionResult, ApprovalRef, CommandEnvelope, Decision, Event, RemediationProposal,
    RemediationRecord, Template,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::command::CommandSigner;
use crate::db;
use crate::policy::PolicyDecision;

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

// ── Dedup key ────────────────────────────────────────────────────────────────

/// The tuple used for fast in-memory deduplication of pending proposals.
/// Uses a stable JSON encoding of `params` to avoid BTreeMap ordering issues.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DedupKey {
    template_id: String,
    agent_id: Uuid,
    params_json: String,
}

impl DedupKey {
    fn from_proposal(p: &RemediationProposal) -> Self {
        let params_json =
            serde_json::to_string(&p.params).unwrap_or_default();
        Self {
            template_id: p.template_id.clone(),
            agent_id: p.agent_id.0,
            params_json,
        }
    }
}

// ── RemediationStore ─────────────────────────────────────────────────────────

/// Postgres-backed audit store for remediation lifecycle (#143).
///
/// The source of truth is the `remediation_records` table. A small in-memory
/// `pending_keys` set guards the hot ingest path against duplicate inserts for
/// a recurring fault without a round-trip to Postgres. The set is rebuilt from
/// Postgres on construction (see [`RemediationStore::new`]) and kept in sync
/// with every write.
pub struct RemediationStore {
    pool: PgPool,
    /// Dedup guard: DedupKey of every proposal currently in `decision_state = 'pending'`.
    pending_keys: Mutex<BTreeSet<DedupKey>>,
}

impl RemediationStore {
    /// Wrap an existing pool, rebuilding the in-memory dedup cache from live
    /// pending rows. Call this once at startup after migrations have run.
    pub async fn new(pool: PgPool) -> anyhow::Result<Self> {
        // Rebuild the dedup set from any rows that survived a restart.
        let records = db::list_remediation_records(&pool).await?;
        let keys: BTreeSet<DedupKey> = records
            .iter()
            .filter(|r| matches!(r.decision, Decision::Pending))
            .map(|r| DedupKey::from_proposal(&r.proposal))
            .collect();
        tracing::info!(
            pending = keys.len(),
            total = records.len(),
            "remediation store initialised from Postgres"
        );
        Ok(Self { pool, pending_keys: Mutex::new(keys) })
    }

    /// Insert a new pending proposal. Returns the proposal id if inserted, or
    /// `None` if an identical pending proposal already exists (dedup). Writes
    /// to Postgres immediately; the dedup cache is updated on success.
    pub async fn insert(
        &self,
        proposal: RemediationProposal,
        fault_signature: String,
    ) -> Option<Uuid> {
        let key = DedupKey::from_proposal(&proposal);
        {
            let keys = self.pending_keys.lock().expect("dedup lock poisoned");
            if keys.contains(&key) {
                return None;
            }
        }
        let id = proposal.id;
        match db::insert_remediation(&self.pool, &proposal, &fault_signature).await {
            Ok(()) => {
                self.pending_keys.lock().expect("dedup lock poisoned").insert(key);
                tracing::debug!(proposal = %id, "remediation inserted into Postgres");
                Some(id)
            }
            Err(e) => {
                tracing::warn!(error = %e, proposal = %id, "failed to insert remediation into Postgres");
                None
            }
        }
    }

    /// Insert a policy-auto-approved record (already `approved` state). Returns
    /// the proposal id, or `None` if an identical *pending* proposal already
    /// exists. Does not add to the pending dedup cache (it is already approved).
    pub async fn insert_auto_approved(
        &self,
        proposal: RemediationProposal,
        command_id: Uuid,
        signature: Option<String>,
        fault_signature: String,
    ) -> Option<Uuid> {
        let key = DedupKey::from_proposal(&proposal);
        {
            let keys = self.pending_keys.lock().expect("dedup lock poisoned");
            if keys.contains(&key) {
                return None;
            }
        }
        let id = proposal.id;
        let decision = Decision::Approved { by: ApprovalRef::PolicyAuto };
        match db::insert_auto_approved_remediation(
            &self.pool,
            &proposal,
            &decision,
            command_id,
            signature.as_deref(),
            &fault_signature,
        )
        .await
        {
            Ok(()) => {
                tracing::debug!(proposal = %id, %command_id, "auto-approved remediation inserted into Postgres");
                Some(id)
            }
            Err(e) => {
                tracing::warn!(error = %e, proposal = %id, "failed to insert auto-approved remediation");
                None
            }
        }
    }

    /// All records, newest first (reads from Postgres).
    pub async fn list(&self) -> Vec<RemediationRecord> {
        match db::list_remediation_records(&self.pool).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "failed to list remediations from Postgres");
                Vec::new()
            }
        }
    }

    /// Return the proposal for a pending record, or `None`.
    pub async fn pending_proposal(&self, id: Uuid) -> Option<RemediationProposal> {
        match db::pending_remediation_proposal(&self.pool, id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, proposal = %id, "failed to fetch pending proposal");
                None
            }
        }
    }

    /// Record approval and the issued signed command. Removes the proposal from
    /// the pending dedup cache. Returns `true` if a pending row was found.
    pub async fn approve(
        &self,
        id: Uuid,
        by: ApprovalRef,
        command_id: Uuid,
        signature: Option<String>,
    ) -> bool {
        // Capture the dedup key before the state transition so we can evict it.
        let maybe_key = db::pending_remediation_proposal(&self.pool, id)
            .await
            .ok()
            .flatten()
            .map(|p| DedupKey::from_proposal(&p));

        let decision = Decision::Approved { by };
        match db::approve_remediation(&self.pool, id, &decision, command_id, signature.as_deref()).await {
            Ok(found) => {
                if found {
                    if let Some(key) = maybe_key {
                        self.pending_keys.lock().expect("dedup lock poisoned").remove(&key);
                    }
                }
                found
            }
            Err(e) => {
                tracing::error!(error = %e, proposal = %id, "failed to approve remediation in Postgres");
                false
            }
        }
    }

    /// Record a rejection. Returns `true` if a record was found and updated.
    pub async fn reject(&self, id: Uuid, by: String, reason: Option<String>) -> bool {
        // Capture the dedup key before the state transition so we can evict it.
        let maybe_key = db::pending_remediation_proposal(&self.pool, id)
            .await
            .ok()
            .flatten()
            .map(|p| DedupKey::from_proposal(&p));

        let decision = Decision::Rejected { by, at: Utc::now(), reason };
        match db::reject_remediation(&self.pool, id, &decision).await {
            Ok(found) => {
                if found {
                    if let Some(key) = maybe_key {
                        self.pending_keys.lock().expect("dedup lock poisoned").remove(&key);
                    }
                }
                found
            }
            Err(e) => {
                tracing::error!(error = %e, proposal = %id, "failed to reject remediation in Postgres");
                false
            }
        }
    }

    /// Attach the agent-reported result and return the closed record + fault
    /// signature (for knowledge-base update). `None` if no record matches.
    pub async fn record_result(
        &self,
        result: ActionResult,
    ) -> Option<(RemediationRecord, String)> {
        match db::record_remediation_result(&self.pool, &result).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, command_id = %result.command_id, "failed to record remediation result");
                None
            }
        }
    }
}

// ── prepare ──────────────────────────────────────────────────────────────────

/// Prepare hook, called best-effort after a message is persisted (#115). Matches
/// a template and resolves parameters, then asks the policy engine (#116) what
/// to do: `auto` signs + enqueues the command immediately and records it as
/// approved-by-[`ApprovalRef::PolicyAuto`]; `approve` records a pending proposal
/// for a human (the default); `forbid` produces nothing. Never blocks or errors
/// the ingestion path.
///
/// Spawns an async task; returns immediately so the NATS ingest loop is not
/// delayed by DB I/O.
pub fn prepare(state: &crate::state::AppState, message: &ravn_core::Message) {
    let event = message.event.clone();
    let state = state.clone();
    tokio::spawn(async move {
        prepare_inner(&state, &event).await;
    });
}

async fn prepare_inner(state: &crate::state::AppState, event: &Event) {
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
            if let Some(id) = state.remediations.insert(proposal, signature).await {
                tracing::info!(proposal = %id, template = %template_id, host = %event.host, "remediation proposed (awaiting approval)");
            }
        }
        PolicyDecision::Auto => {
            auto_execute(state, template, proposal, &template_id, &event.host, signature).await
        }
    }
}

/// Sign, enqueue, and record a policy-auto-approved remediation. Best-effort: a
/// build failure is logged and dropped (it must never disturb ingestion).
async fn auto_execute(
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
    if let Some(id) = state
        .remediations
        .insert_auto_approved(proposal, command_id, signature, fault_signature)
        .await
    {
        state.command_queue.enqueue(envelope);
        tracing::info!(proposal = %id, %command_id, template = %template_id, host, "remediation auto-executed by policy");
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{AgentId, Capability, FailedUnitPayload, Payload, Severity, Source};
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

    // ── Unit tests for the pure helpers (no DB needed) ─────────────────────

    #[test]
    fn dedup_key_is_stable_across_proposals_for_same_fault() {
        let reg = registry();
        let event = failed_unit_event("nginx.service");
        let tpl = reg.match_event(&event).unwrap();
        let p1 = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let p2 = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        // The two proposals have different UUIDs but must map to the same dedup key.
        assert_ne!(p1.id, p2.id, "proposals must have unique ids");
        assert_eq!(DedupKey::from_proposal(&p1), DedupKey::from_proposal(&p2));
    }

    // ── E2E test (requires a live Postgres) ───────────────────────────────
    //
    // This test documents the full lifecycle and the restart-durability contract.
    // It is marked `#[ignore]` so it is skipped in the hermetic Nix build. Run
    // it manually against a live database:
    //
    //   DATABASE_URL=postgres://ravn:ravn@localhost/ravn \
    //       cargo test -p ravn-server -- --ignored audit_trail_survives_restart
    //
    // What it verifies:
    //   1. Insert a proposal → row appears in Postgres.
    //   2. Approve it → row transitions to "approved".
    //   3. Drop the store (simulating a server restart), rebuild from DB.
    //   4. The record is still there and shows "approved".
    //   5. Record a result → row shows "approved" + result.
    #[tokio::test]
    #[ignore = "requires live Postgres — run with DATABASE_URL set"]
    async fn audit_trail_survives_restart() {
        use ravn_core::ActionStatus;
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for this test");
        let pool = crate::db::connect(&url).await.expect("connect");
        crate::db::migrate(&pool).await.expect("migrate");

        let reg = registry();
        let event = failed_unit_event("postgres-e2e-unit.service");
        let tpl = reg.match_event(&event).unwrap();
        let proposal = build_proposal(tpl, &event, resolve_params(tpl, &event).unwrap());
        let proposal_id = proposal.id;

        // -- Step 1: create the store and insert a proposal.
        let store = RemediationStore::new(pool.clone()).await.expect("store");
        let inserted = store.insert(proposal.clone(), "e2e:sig".into()).await;
        assert!(inserted.is_some(), "proposal must be inserted");

        // -- Step 2: approve it.
        let command_id = Uuid::now_v7();
        let approved = store
            .approve(
                proposal_id,
                ApprovalRef::Human { user: "e2e-test".into(), approved_at: Utc::now() },
                command_id,
                Some("fake-sig".into()),
            )
            .await;
        assert!(approved, "approval must succeed");

        // -- Step 3: simulate restart by building a new store from the same DB.
        let store2 = RemediationStore::new(pool.clone()).await.expect("store2");

        // -- Step 4: the approved record is visible in the new store.
        let records = store2.list().await;
        let rec = records.iter().find(|r| r.proposal.id == proposal_id)
            .expect("record must survive restart");
        assert!(
            matches!(rec.decision, Decision::Approved { .. }),
            "decision must be Approved after restart, got {:?}", rec.decision
        );
        assert_eq!(rec.command_id, Some(command_id));

        // -- Step 5: record a result.
        let result = ActionResult {
            command_id,
            status: ActionStatus::Succeeded,
            detail: None,
            observed_state: Some("active".into()),
            finished_at: Utc::now(),
        };
        let outcome = store2.record_result(result).await;
        assert!(outcome.is_some(), "result must be recorded");
        let (closed, _sig) = outcome.unwrap();
        assert_eq!(closed.result.unwrap().status, ActionStatus::Succeeded);
    }
}
