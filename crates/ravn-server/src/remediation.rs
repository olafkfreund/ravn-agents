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
//!
//! # Condition path validation (#151)
//!
//! Template `match.conditions` and `parameters[*].from` are dotted paths into a
//! serialized [`Event`]. Both are validated at load time against the set of paths
//! that the typed accessor [`event_field`] recognises. An unknown path causes
//! [`TemplateRegistry::load_dir`] to return an error, aborting server startup
//! with a clear message instead of silently never matching.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use ravn_core::{
    is_valid_k8s_label, ActionResult, ApprovalRef, CommandEnvelope, Decision, Event, Payload,
    RemediationProposal, RemediationRecord, Template,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::command::CommandSigner;
use crate::db;
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
        // Self-observability event (#149) — never produced by an agent, so it
        // never reaches template matching, but the kind string is defined for
        // completeness.
        Payload::CircuitBreakerTrip(_) => "circuit_breaker_trip",
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

/// Parameter names that carry Kubernetes identifiers and must conform to RFC 1123
/// DNS-label rules. Any template parameter whose name appears in this set will
/// be validated before the proposal is built — a hostile or malformed value
/// from a Kubernetes event is rejected here, before it can reach the K8s API.
const K8S_ID_PARAM_NAMES: &[&str] = &["namespace", "pod", "deployment", "name"];

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
/// [`event_field`] accessor (#151) — no JSON roundtrip. Parameter values whose
/// names are listed in [`K8S_ID_PARAM_NAMES`] are additionally validated against
/// RFC 1123 DNS-label rules (#145) before being accepted, guarding the K8s API
/// surface against injection-shaped values.
pub fn resolve_params(template: &Template, event: &Event) -> anyhow::Result<BTreeMap<String, String>> {
    let mut params = BTreeMap::new();
    for (name, spec) in &template.parameters {
        let value = event_field(event, spec.from.as_str())
            .ok_or_else(|| anyhow::anyhow!("parameter {name}: no string value at '{}'", spec.from))?;

        // Validate Kubernetes identifiers before they can reach any API call (#145).
        if K8S_ID_PARAM_NAMES.contains(&name.as_str()) && !is_valid_k8s_label(value) {
            anyhow::bail!(
                "parameter {name}: value {value:?} is not a valid Kubernetes identifier \
                 (RFC 1123 DNS label: 1–63 lowercase alphanumeric or '-' chars, \
                 must start and end with alphanumeric)"
            );
        }

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
        kid: None, // stamped by `signer.sign` with the active key's id (#150)
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
///
/// Knowledge base (#118): on a *matched* fault the proposal's rationale is biased
/// with a deterministic recall note ("last N× this fired, template X succeeded");
/// on an *unmatched* fault a `gap` entry is written so operators can see which
/// faults still need a template.
///
/// **#149:** metrics are emitted for every policy decision and every closed
/// action result; a self-observability Ravn event is published when the circuit
/// breaker trips so the portal shows the alert.
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

    let result = state.policy.evaluate(&event.host, template.risk_tier, &template_id, Utc::now());

    // #149: emit the circuit-breaker trip metric + a self-observability Ravn
    // event so the portal shows the alert and Grafana can graph it.
    if result.breaker_tripped {
        state.metrics.circuit_breaker_trips.inc();
        emit_breaker_trip_event(state, &event.host, &template_id);
    }

    match result.decision {
        PolicyDecision::Forbid => {
            tracing::info!(template = %template_id, host = %event.host, "remediation forbidden by policy");
            state.metrics.record_remediation_proposed("forbid");
        }
        PolicyDecision::Approve => {
            state.metrics.record_remediation_proposed("approve");
            if let Some(id) = state.remediations.insert(proposal, signature).await {
                tracing::info!(proposal = %id, template = %template_id, host = %event.host, "remediation proposed (awaiting approval)");
            }
        }
        PolicyDecision::Auto => {
            state.metrics.record_remediation_proposed("auto");
            auto_execute(state, template, proposal, &template_id, &event.host, signature).await
        }
    }
}

/// Emit a synthetic Ravn event that records a circuit-breaker trip as a
/// first-class fleet event (#149). Self-observability: Ravn alerts on itself.
/// Best-effort — never panics or blocks the ingestion path.
fn emit_breaker_trip_event(state: &crate::state::AppState, host: &str, template_id: &str) {
    use ravn_core::{AgentId, Event, Payload, Severity};
    use uuid::Uuid;

    // A zero agent-id marks this as a synthetic control-plane event.
    let agent_id = AgentId(Uuid::nil());
    let now = Utc::now();
    let event = Event {
        id: Uuid::now_v7(),
        occurred_at: now,
        observed_at: now,
        agent_id,
        host: host.to_string(),
        severity: Severity::Warning,
        title: format!("Circuit breaker tripped: auto→approve for template {template_id} on {host}"),
        category_hints: vec!["ravn:self".to_string(), "circuit-breaker".to_string()],
        payload: Payload::CircuitBreakerTrip(ravn_core::CircuitBreakerTripPayload {
            host: host.to_string(),
            template_id: template_id.to_string(),
        }),
    };
    // Fan the event to WebSocket subscribers immediately (no DB persist needed
    // for an in-memory self-observability ping).
    let stored = crate::db::synthetic_event_to_stored(&event);
    let _ = state.events_tx.send(stored);
    tracing::warn!(host, template = template_id, "circuit breaker tripped — emitting self-observability event");
}

/// Sign, enqueue, and record a policy-auto-approved remediation. Best-effort: a
/// build failure is logged and dropped (it must never disturb ingestion).
/// #149: build failures increment `ravn_command_channel_errors_total`.
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
            state.metrics.command_channel_errors.inc();
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
    use ravn_core::{
        AgentId, Capability, FailedUnitPayload, KubeWorkloadPayload, ParameterSpec, ParamType,
        Payload, RiskTier, Rollback, Severity, Source, Template, TemplateMatch,
    };
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

    // Regression (#121 VM e2e): the generic failed-unit template must heal a unit
    // that entered `failed` for ANY systemd result, not just "exit-code". A
    // SIGKILL reports result "signal"; a too-narrow condition silently skipped it.
    #[test]
    fn generic_template_matches_a_signal_killed_unit() {
        let mut event = failed_unit_event("dummy.service");
        if let Payload::FailedUnit(p) = &mut event.payload {
            p.result = "signal".into();
        }
        let reg = registry();
        let tpl = reg.match_event(&event).expect("a signal-killed unit must still match");
        assert_eq!(tpl.id, "failed-unit-restart");
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

    // ── K8s identifier validation in resolve_params (issue #145) ─────────

    /// Build a minimal K8s pod-restart template with `namespace` and `pod`
    /// parameters extracted from `payload.namespace` and `payload.name`.
    fn k8s_pod_restart_template() -> Template {
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "namespace".to_string(),
            ParameterSpec { ty: ParamType::String, from: "payload.namespace".to_string() },
        );
        parameters.insert(
            "pod".to_string(),
            ParameterSpec { ty: ParamType::String, from: "payload.name".to_string() },
        );
        Template {
            id: "k8s-pod-restart".to_string(),
            version: 1,
            title: "Restart a crashed Kubernetes pod".to_string(),
            risk_tier: RiskTier::Guarded,
            match_: TemplateMatch {
                source: Source::KubeWorkload,
                conditions: BTreeMap::new(),
            },
            parameters,
            preconditions: vec![],
            steps: vec![Capability::DeletePod {
                namespace: "{{namespace}}".to_string(),
                pod: "{{pod}}".to_string(),
            }],
            verify: None,
            rollback: Rollback::None,
        }
    }

    fn kube_workload_event(namespace: &str, pod_name: &str) -> Event {
        let now = Utc::now();
        Event {
            id: uuid::Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(uuid::Uuid::now_v7()),
            host: "k8s-controller".into(),
            severity: Severity::Error,
            title: format!("{pod_name} crashlooping"),
            category_hints: vec![],
            payload: Payload::KubeWorkload(KubeWorkloadPayload {
                namespace: namespace.into(),
                name: pod_name.into(),
                object_kind: "Pod".into(),
                reason: "CrashLoopBackOff".into(),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn valid_k8s_params_resolve_successfully() {
        let tpl = k8s_pod_restart_template();
        let event = kube_workload_event("payments", "api-7d9f");
        let params = resolve_params(&tpl, &event).unwrap();
        assert_eq!(params.get("namespace").map(String::as_str), Some("payments"));
        assert_eq!(params.get("pod").map(String::as_str), Some("api-7d9f"));
    }

    #[test]
    fn uppercase_namespace_in_event_is_rejected_at_resolve() {
        let tpl = k8s_pod_restart_template();
        let event = kube_workload_event("Payments", "api-7d9f");
        let err = resolve_params(&tpl, &event).unwrap_err();
        assert!(err.to_string().contains("namespace"), "error must name the offending field");
        assert!(err.to_string().contains("Payments"), "error must include the bad value");
    }

    #[test]
    fn injection_pod_name_in_event_is_rejected_at_resolve() {
        let tpl = k8s_pod_restart_template();
        let event = kube_workload_event("payments", "api; curl evil.com");
        let err = resolve_params(&tpl, &event).unwrap_err();
        assert!(err.to_string().contains("pod"));
    }

    #[test]
    fn path_traversal_namespace_is_rejected_at_resolve() {
        let tpl = k8s_pod_restart_template();
        let event = kube_workload_event("../etc", "api");
        let err = resolve_params(&tpl, &event).unwrap_err();
        assert!(err.to_string().contains("namespace"));
    }

    #[test]
    fn empty_namespace_in_event_is_rejected_at_resolve() {
        let tpl = k8s_pod_restart_template();
        let event = kube_workload_event("", "api");
        let err = resolve_params(&tpl, &event).unwrap_err();
        assert!(err.to_string().contains("namespace"));
    }

    #[test]
    fn overly_long_pod_name_in_event_is_rejected_at_resolve() {
        let tpl = k8s_pod_restart_template();
        let long_name = "a".repeat(64);
        let event = kube_workload_event("payments", &long_name);
        let err = resolve_params(&tpl, &event).unwrap_err();
        assert!(err.to_string().contains("pod"));
    }

    #[test]
    fn whitespace_in_k8s_id_in_event_is_rejected_at_resolve() {
        let tpl = k8s_pod_restart_template();
        let event = kube_workload_event("pay ments", "api");
        let err = resolve_params(&tpl, &event).unwrap_err();
        assert!(err.to_string().contains("namespace"));
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
