//! Remediation knowledge base (#118) — the *Reflect* half of the PARR loop.
//!
//! A per-environment markdown wiki, one file per fault signature, that the
//! control plane writes deterministically as remediations resolve and reads back
//! to bias the next proposal. There is **no embedding model and no vector store**
//! (in-doctrine, CPU-light): recall is an exact-signature lookup, and the
//! retrospective body is templated, never LLM-authored.
//!
//! Each entry is a markdown file with a hand-rolled YAML front-matter block:
//!
//! ```markdown
//! ---
//! fault_signature: "FailedUnit:nginx.service:failed"
//! template_used: failed-unit-restart@3
//! params: { unit: nginx.service }
//! outcomes:
//!   - { ts: 2026-06-05T10:02:00Z, result: succeeded, ttr_s: 8, approver: "olaf" }
//! occurrences: 14
//! last_seen: 2026-06-05T10:02:00Z
//! ---
//! # FailedUnit:nginx.service:failed
//! ...templated retrospective...
//! ```
//!
//! The KB is disabled (a no-op) when `RAVN_KB_DIR` is unset, so P1 behaviour is
//! unchanged. Writes use a temp-file + rename so a crash never leaves a partial
//! entry; no real git is required (operators commit the directory out of band).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use ravn_core::{ActionStatus, Event, Payload, RemediationProposal};

/// The `result` field recorded per outcome, mirroring [`ActionStatus`] in the
/// lowercase form used elsewhere on the wire.
fn status_str(status: ActionStatus) -> &'static str {
    match status {
        ActionStatus::Succeeded => "succeeded",
        ActionStatus::Failed => "failed",
        ActionStatus::Rejected => "rejected",
    }
}

/// A deterministic fault signature, `"<Source>:<unit-or-key>:<state>"`, reused
/// for both writing entries and recall. The middle key identifies the affected
/// object (a unit, path, or workload) and the trailing state describes the
/// fault, so recurrences of the same fault share one entry.
pub fn fault_signature(event: &Event) -> String {
    let source = format!("{:?}", event.source());
    let (key, state) = match &event.payload {
        Payload::FailedUnit(p) => (p.unit.clone(), p.result.clone()),
        Payload::Journald(p) => (p.unit.clone().unwrap_or_else(|| "-".into()), "journal".into()),
        Payload::ConfigDrift(p) => (p.path.clone(), "drift".into()),
        Payload::Auth(p) => (
            p.user.clone().unwrap_or_else(|| "-".into()),
            p.action.clone(),
        ),
        Payload::Update(p) => (p.mechanism.clone(), "update".into()),
        Payload::KubeWorkload(p) => {
            let key = if p.namespace.is_empty() {
                p.name.clone()
            } else {
                format!("{}/{}", p.namespace, p.name)
            };
            (key, p.reason.clone())
        }
        Payload::KubeNode(p) => (p.node.clone(), p.condition.clone()),
    };
    format!("{source}:{key}:{state}")
}

/// Slugify a fault signature into a safe markdown filename stem (no path
/// separators, colons, or whitespace). Distinct signatures map to distinct
/// slugs because every non-alphanumeric run collapses to a single `-` and the
/// original separators (`:`, `/`) are all replaced uniformly.
fn slug(signature: &str) -> String {
    let mut out = String::with_capacity(signature.len());
    let mut last_dash = false;
    for ch in signature.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// A single recorded outcome appended to an entry's `outcomes` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub ts: DateTime<Utc>,
    /// Lowercase [`ActionStatus`], e.g. `succeeded`.
    pub result: String,
    /// Time-to-recover in seconds (approval → reported finish), when known.
    pub ttr_s: Option<i64>,
    /// Who approved the remediation (operator id, or `policy:auto`).
    pub approver: String,
}

/// A parsed knowledge-base entry: front-matter plus the markdown body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub fault_signature: String,
    /// `template-id@version`, or `gap` for an unhandled fault.
    pub template_used: String,
    /// Resolved parameters of the remediation (empty for gap entries).
    pub params: std::collections::BTreeMap<String, String>,
    pub outcomes: Vec<Outcome>,
    pub occurrences: u64,
    pub last_seen: DateTime<Utc>,
    /// Templated, human-readable retrospective.
    pub body: String,
}

impl Entry {
    /// Render the entry to its on-disk markdown form (front-matter + body).
    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("---\n");
        s.push_str(&format!("fault_signature: \"{}\"\n", self.fault_signature));
        s.push_str(&format!("template_used: {}\n", self.template_used));
        s.push_str(&format!("params: {{ {} }}\n", render_params(&self.params)));
        if self.outcomes.is_empty() {
            s.push_str("outcomes: []\n");
        } else {
            s.push_str("outcomes:\n");
            for o in &self.outcomes {
                let ttr = o.ttr_s.map(|v| v.to_string()).unwrap_or_else(|| "null".into());
                s.push_str(&format!(
                    "  - {{ ts: {}, result: {}, ttr_s: {}, approver: \"{}\" }}\n",
                    o.ts.to_rfc3339(),
                    o.result,
                    ttr,
                    o.approver,
                ));
            }
        }
        s.push_str(&format!("occurrences: {}\n", self.occurrences));
        s.push_str(&format!("last_seen: {}\n", self.last_seen.to_rfc3339()));
        s.push_str("---\n");
        s.push_str(&self.body);
        if !self.body.ends_with('\n') {
            s.push('\n');
        }
        s
    }

    /// Parse an entry from its on-disk markdown form. Tolerant of the exact
    /// shape [`Entry::to_markdown`] produces; unknown front-matter keys are
    /// ignored so the format can grow.
    pub fn from_markdown(text: &str) -> anyhow::Result<Self> {
        let rest = text
            .strip_prefix("---\n")
            .ok_or_else(|| anyhow::anyhow!("entry missing front-matter opener"))?;
        let (front, body) = rest
            .split_once("\n---\n")
            .ok_or_else(|| anyhow::anyhow!("entry missing front-matter closer"))?;

        let mut fault_signature = String::new();
        let mut template_used = String::new();
        let mut params = std::collections::BTreeMap::new();
        let mut outcomes = Vec::new();
        let mut occurrences = 0u64;
        let mut last_seen = Utc::now();

        let mut lines = front.lines().peekable();
        while let Some(line) = lines.next() {
            let line = line.trim_end();
            if let Some(v) = line.strip_prefix("fault_signature:") {
                fault_signature = unquote(v.trim());
            } else if let Some(v) = line.strip_prefix("template_used:") {
                template_used = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("params:") {
                params = parse_params(v.trim());
            } else if let Some(v) = line.strip_prefix("occurrences:") {
                occurrences = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("last_seen:") {
                if let Ok(ts) = DateTime::parse_from_rfc3339(v.trim()) {
                    last_seen = ts.with_timezone(&Utc);
                }
            } else if line.trim() == "outcomes:" {
                // Block list: consume the indented `- { ... }` lines.
                while let Some(peek) = lines.peek() {
                    let t = peek.trim_start();
                    if let Some(item) = t.strip_prefix("- ") {
                        if let Some(o) = parse_outcome(item.trim()) {
                            outcomes.push(o);
                        }
                        lines.next();
                    } else {
                        break;
                    }
                }
            } else if let Some(v) = line.strip_prefix("outcomes: [") {
                // Empty inline list `outcomes: []`.
                let _ = v;
            }
        }

        Ok(Entry {
            fault_signature,
            template_used,
            params,
            outcomes,
            occurrences,
            last_seen,
            body: body.to_string(),
        })
    }
}

/// Render a params map as the inline `k: v, k: v` form used in front-matter.
fn render_params(params: &std::collections::BTreeMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse the inline `{ k: v, k: v }` params form back into a map.
fn parse_params(raw: &str) -> std::collections::BTreeMap<String, String> {
    let inner = raw.trim().trim_start_matches('{').trim_end_matches('}').trim();
    let mut map = std::collections::BTreeMap::new();
    if inner.is_empty() {
        return map;
    }
    for pair in inner.split(',') {
        if let Some((k, v)) = pair.split_once(':') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// Parse one `{ ts: ..., result: ..., ttr_s: ..., approver: "..." }` outcome.
fn parse_outcome(raw: &str) -> Option<Outcome> {
    let inner = raw.trim().trim_start_matches('{').trim_end_matches('}').trim();
    let mut ts = None;
    let mut result = None;
    let mut ttr_s = None;
    let mut approver = String::new();
    for pair in split_top_level(inner) {
        let (k, v) = pair.split_once(':')?;
        let (k, v) = (k.trim(), v.trim());
        match k {
            "ts" => ts = DateTime::parse_from_rfc3339(v).ok().map(|t| t.with_timezone(&Utc)),
            "result" => result = Some(v.to_string()),
            "ttr_s" => ttr_s = if v == "null" { None } else { v.parse().ok() },
            "approver" => approver = unquote(v),
            _ => {}
        }
    }
    Some(Outcome { ts: ts?, result: result?, ttr_s, approver })
}

/// Split on top-level commas (the RFC3339 timestamp itself contains none, but
/// this keeps the parser robust to any future quoted commas).
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    let mut in_quote = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b'{' | b'[' if !in_quote => depth += 1,
            b'}' | b']' if !in_quote => depth -= 1,
            b',' if depth == 0 && !in_quote => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// The knowledge base: a directory of markdown entries, or disabled (`None`).
#[derive(Default)]
pub struct KnowledgeBase {
    dir: Option<PathBuf>,
}

impl KnowledgeBase {
    /// Open the KB rooted at `dir`, creating it if needed. `None` disables the
    /// KB entirely (every method becomes a no-op), preserving P1 behaviour.
    pub fn open(dir: Option<&str>) -> anyhow::Result<Self> {
        match dir {
            None => Ok(Self::default()),
            Some(dir) => {
                let path = PathBuf::from(dir);
                std::fs::create_dir_all(&path)
                    .map_err(|e| anyhow::anyhow!("creating KB dir {dir}: {e}"))?;
                tracing::info!(dir = %path.display(), "remediation knowledge base enabled");
                Ok(Self { dir: Some(path) })
            }
        }
    }

    /// Whether the KB is enabled.
    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Path of the entry file for a fault signature.
    fn entry_path(&self, signature: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}.md", slug(signature))))
    }

    /// Read the entry for a signature, if one exists.
    pub fn load(&self, signature: &str) -> Option<Entry> {
        let path = self.entry_path(signature)?;
        let text = std::fs::read_to_string(&path).ok()?;
        match Entry::from_markdown(&text) {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::warn!(%e, path = %path.display(), "ignoring unparseable KB entry");
                None
            }
        }
    }

    /// Atomically write an entry (temp file + rename).
    fn store(&self, entry: &Entry) -> anyhow::Result<()> {
        let Some(path) = self.entry_path(&entry.fault_signature) else { return Ok(()) };
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, entry.to_markdown())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// A short, deterministic recall note for a fault signature, or `None` when
    /// the KB is disabled or has no prior (successful) record. Surfaced in the
    /// proposal rationale to bias the operator's decision.
    pub fn recall(&self, signature: &str) -> Option<String> {
        let entry = self.load(signature)?;
        if entry.template_used == "gap" || entry.outcomes.is_empty() {
            return None;
        }
        let successes = entry
            .outcomes
            .iter()
            .filter(|o| o.result == "succeeded")
            .collect::<Vec<_>>();
        if successes.is_empty() {
            return None;
        }
        let ttrs: Vec<i64> = successes.iter().filter_map(|o| o.ttr_s).collect();
        let avg = if ttrs.is_empty() {
            String::new()
        } else {
            format!(", avg {}s", ttrs.iter().sum::<i64>() / ttrs.len() as i64)
        };
        Some(format!(
            "Recall: last {} time(s) this fault fired, template {} succeeded {}/{}{}.",
            entry.occurrences,
            entry.template_used,
            successes.len(),
            entry.outcomes.len(),
            avg,
        ))
    }

    /// Record a resolved remediation: create or update the entry for the fault,
    /// appending the outcome and bumping occurrences. No-op when disabled.
    pub fn record_outcome(
        &self,
        signature: &str,
        proposal: &RemediationProposal,
        status: ActionStatus,
        ttr_s: Option<i64>,
        approver: &str,
    ) {
        if !self.is_enabled() {
            return;
        }
        let now = Utc::now();
        let template_used = format!("{}@{}", proposal.template_id, proposal.template_version);
        let outcome = Outcome {
            ts: now,
            result: status_str(status).to_string(),
            ttr_s,
            approver: approver.to_string(),
        };

        let mut entry = self.load(signature).unwrap_or_else(|| Entry {
            fault_signature: signature.to_string(),
            template_used: template_used.clone(),
            params: proposal.params.clone(),
            outcomes: Vec::new(),
            occurrences: 0,
            last_seen: now,
            body: String::new(),
        });
        entry.template_used = template_used;
        entry.params = proposal.params.clone();
        entry.outcomes.push(outcome);
        entry.occurrences += 1;
        entry.last_seen = now;
        entry.body = retrospective_body(signature, &entry);

        if let Err(e) = self.store(&entry) {
            tracing::warn!(%e, signature, "failed to write KB entry");
        } else {
            tracing::info!(signature, occurrences = entry.occurrences, "knowledge base updated");
        }
    }

    /// Record a `gap`: an event with no matching template. Operators read these
    /// to see which faults still need a template authored. Does not call any
    /// external API. No-op when disabled.
    pub fn record_gap(&self, event: &Event) {
        if !self.is_enabled() {
            return;
        }
        let signature = fault_signature(event);
        let now = Utc::now();
        let mut entry = self.load(&signature).unwrap_or_else(|| Entry {
            fault_signature: signature.clone(),
            template_used: "gap".to_string(),
            params: std::collections::BTreeMap::new(),
            outcomes: Vec::new(),
            occurrences: 0,
            last_seen: now,
            body: String::new(),
        });
        // Don't clobber a real entry that later regressed to no-match.
        if entry.template_used.is_empty() {
            entry.template_used = "gap".to_string();
        }
        entry.occurrences += 1;
        entry.last_seen = now;
        if entry.body.is_empty() {
            entry.body = gap_body(&signature, event);
        }

        if let Err(e) = self.store(&entry) {
            tracing::warn!(%e, signature, "failed to write KB gap entry");
        } else {
            tracing::info!(signature, "knowledge base gap recorded (no matching template)");
        }
    }
}

/// Deterministic, templated retrospective body for a resolved fault. (No LLM.)
fn retrospective_body(signature: &str, entry: &Entry) -> String {
    let succeeded = entry.outcomes.iter().filter(|o| o.result == "succeeded").count();
    format!(
        "# {signature}\n\n\
         **Reflect:** template `{}` has been applied {} time(s) to this fault, \
         succeeding {}/{}. Recurrence may indicate an upstream cause worth a deeper look.\n",
        entry.template_used,
        entry.occurrences,
        succeeded,
        entry.outcomes.len(),
    )
}

/// Deterministic, templated body for a gap (unhandled fault) entry.
fn gap_body(signature: &str, event: &Event) -> String {
    format!(
        "# {signature}\n\n\
         **Gap:** no remediation template matched this fault on `{}`. \
         Author a template so the control plane can remediate it next time.\n\n\
         Event title: {}\n",
        event.host, event.title,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{AgentId, FailedUnitPayload, RiskTier, Severity};
    use std::path::Path;
    use uuid::Uuid;

    /// A unique temp directory under the system temp dir, removed on drop. Keeps
    /// the KB tests off any real git checkout and free of a `tempfile` dep.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ravn-kb-test-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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

    fn proposal_for(event: &Event) -> RemediationProposal {
        let mut params = std::collections::BTreeMap::new();
        params.insert("unit".to_string(), "nginx.service".to_string());
        RemediationProposal {
            id: Uuid::now_v7(),
            event_id: event.id,
            agent_id: event.agent_id,
            host: event.host.clone(),
            template_id: "failed-unit-restart".into(),
            template_version: 3,
            risk_tier: RiskTier::Safe,
            params,
            rationale: "test".into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn fault_signature_is_source_key_state() {
        let event = failed_unit_event("nginx.service");
        assert_eq!(fault_signature(&event), "FailedUnit:nginx.service:exit-code");
    }

    #[test]
    fn entry_front_matter_round_trips() {
        let now = Utc::now();
        let mut params = std::collections::BTreeMap::new();
        params.insert("unit".to_string(), "nginx.service".to_string());
        let entry = Entry {
            fault_signature: "FailedUnit:nginx.service:exit-code".into(),
            template_used: "failed-unit-restart@3".into(),
            params,
            outcomes: vec![Outcome {
                ts: now,
                result: "succeeded".into(),
                ttr_s: Some(8),
                approver: "olaf".into(),
            }],
            occurrences: 14,
            last_seen: now,
            body: "# FailedUnit:nginx.service:exit-code\n\nbody text\n".into(),
        };
        let md = entry.to_markdown();
        let back = Entry::from_markdown(&md).expect("parse");
        assert_eq!(back.fault_signature, entry.fault_signature);
        assert_eq!(back.template_used, entry.template_used);
        assert_eq!(back.params, entry.params);
        assert_eq!(back.occurrences, 14);
        assert_eq!(back.outcomes.len(), 1);
        assert_eq!(back.outcomes[0].result, "succeeded");
        assert_eq!(back.outcomes[0].ttr_s, Some(8));
        assert_eq!(back.outcomes[0].approver, "olaf");
        assert_eq!(back.body, entry.body);
    }

    #[test]
    fn disabled_kb_is_a_noop() {
        let kb = KnowledgeBase::open(None).unwrap();
        assert!(!kb.is_enabled());
        let event = failed_unit_event("nginx.service");
        kb.record_gap(&event);
        kb.record_outcome(
            &fault_signature(&event),
            &proposal_for(&event),
            ActionStatus::Succeeded,
            Some(5),
            "olaf",
        );
        assert!(kb.recall(&fault_signature(&event)).is_none());
    }

    #[test]
    fn record_outcome_then_recall_biases_rationale() {
        let dir = TempDir::new();
        let kb = KnowledgeBase::open(Some(dir.path().to_str().unwrap())).unwrap();
        let event = failed_unit_event("nginx.service");
        let sig = fault_signature(&event);
        let proposal = proposal_for(&event);

        // No prior — no recall.
        assert!(kb.recall(&sig).is_none());

        kb.record_outcome(&sig, &proposal, ActionStatus::Succeeded, Some(8), "olaf");
        kb.record_outcome(&sig, &proposal, ActionStatus::Succeeded, Some(12), "policy:auto");

        let note = kb.recall(&sig).expect("recall after outcomes");
        assert!(note.contains("failed-unit-restart@3"), "note: {note}");
        assert!(note.contains("succeeded 2/2"), "note: {note}");
        assert!(note.contains("avg 10s"), "note: {note}");

        // Entry persisted with bumped occurrences.
        let entry = kb.load(&sig).unwrap();
        assert_eq!(entry.occurrences, 2);
        assert_eq!(entry.outcomes.len(), 2);
    }

    #[test]
    fn gap_entry_written_for_unmatched_fault() {
        let dir = TempDir::new();
        let kb = KnowledgeBase::open(Some(dir.path().to_str().unwrap())).unwrap();
        let event = failed_unit_event("mystery.service");
        let sig = fault_signature(&event);

        kb.record_gap(&event);

        let entry = kb.load(&sig).expect("gap entry exists");
        assert_eq!(entry.template_used, "gap");
        assert_eq!(entry.occurrences, 1);
        assert!(entry.body.contains("Gap:"), "body: {}", entry.body);
        // A gap produces no recall (nothing to suggest).
        assert!(kb.recall(&sig).is_none());

        // A second occurrence bumps the counter.
        kb.record_gap(&event);
        assert_eq!(kb.load(&sig).unwrap().occurrences, 2);
    }
}
