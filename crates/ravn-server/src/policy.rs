//! Declarative, default-deny remediation policy engine (#116).
//!
//! Prepare (in `remediation.rs`) matches a curated [`Template`] to a fault; this
//! module decides *what happens next* without the LLM ever touching the choice.
//! A per-environment policy file (`RAVN_POLICY_FILE`, optional) maps
//! `{risk_tier × scope}` to one of [`PolicyDecision::Auto`],
//! [`PolicyDecision::Approve`], or [`PolicyDecision::Forbid`]:
//!
//! - `auto` → the control plane immediately signs and enqueues the command and
//!   records the action as approved-by-[`ApprovalRef::PolicyAuto`];
//! - `approve` → the manual-approval path (a pending proposal — the P1 default);
//! - `forbid` → no proposal is produced at all.
//!
//! **Default-deny:** anything not explicitly matched by an `auto` rule requires
//! approval. With no policy file set, every match is `approve` — P1 behaviour is
//! exactly preserved. Three hard guardrails sit on top of the rules:
//!
//! 1. **Kill switch** (`RAVN_REMEDIATION_AUTO`, default off): forces every
//!    decision to `approve`, fleet-wide.
//! 2. **`dangerous` never auto-executes** — a policy that says otherwise is
//!    downgraded to `approve` (defence in depth; the design forbids it outright).
//! 3. **Circuit breaker / rate limit**: a per-`(host, template)` cap on
//!    auto-executions within a sliding window. Once exceeded, further `auto`
//!    decisions for that pair are downgraded to `approve` (escalated to a human)
//!    until the window drains — this kills restart storms and flapping.
//!
//! Policy-file *signing* (so a tampered policy cannot silently widen
//! auto-execution) is a follow-up; this PR covers evaluation, the auto path, the
//! circuit breaker, and the kill switch.
//!
//! **#149:** `evaluate` now returns [`EvaluateResult`] which also carries the
//! `breaker_tripped` flag so the call site can increment the metric and emit a
//! self-observability Ravn event without the engine needing its own I/O.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ravn_core::RiskTier;
use serde::Deserialize;

/// What policy decides should happen to a matched remediation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    /// Sign and enqueue the command immediately (approved by `policy:auto`).
    Auto,
    /// Record a pending proposal for a human to approve (the default).
    Approve,
    /// Produce nothing — this remediation is not permitted in this environment.
    Forbid,
}

/// One declarative policy rule: a `{tier × scope}` match mapped to a decision.
/// Scope is deliberately simple — an optional host glob plus the tier. A rule
/// with no `host` matches every host.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyRule {
    /// Optional host glob (e.g. `web-*`, `*.prod`). Absent → matches any host.
    #[serde(default)]
    pub host: Option<String>,
    /// The risk tier this rule applies to.
    pub tier: RiskTier,
    /// The decision to take when the rule matches.
    pub decision: PolicyDecision,
}

impl PolicyRule {
    /// Whether this rule applies to `(host, tier)`.
    fn matches(&self, host: &str, tier: RiskTier) -> bool {
        self.tier == tier && self.host.as_deref().map(|g| glob_match(g, host)).unwrap_or(true)
    }
}

/// The declarative policy: an ordered list of rules. The **first** matching rule
/// wins; an unmatched `{host, tier}` defaults to [`PolicyDecision::Approve`]
/// (default-deny — nothing auto-executes unless a rule says so).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Policy {
    #[serde(default, rename = "rule")]
    rules: Vec<PolicyRule>,
}

impl Policy {
    /// Load and parse a policy TOML file. A missing file yields the default
    /// (empty) policy — every match becomes `approve`, exactly P1 behaviour.
    pub fn load_file(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let policy: Policy = toml::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("parsing policy {}: {e}", path.display()))?;
                tracing::info!(
                    count = policy.rules.len(),
                    path = %path.display(),
                    "loaded remediation policy"
                );
                Ok(policy)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(
                    path = %path.display(),
                    "no policy file; all remediations require approval (default-deny)"
                );
                Ok(Self::default())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The raw rule decision for `(host, tier)` before any guardrail is applied.
    /// First matching rule wins; no match → `approve` (default-deny).
    fn decide(&self, host: &str, tier: RiskTier) -> PolicyDecision {
        self.rules
            .iter()
            .find(|r| r.matches(host, tier))
            .map(|r| r.decision)
            .unwrap_or(PolicyDecision::Approve)
    }
}

/// The result of [`PolicyEngine::evaluate`].
///
/// The decision is final. `breaker_tripped` is `true` when the circuit
/// breaker was the reason an `auto` rule was downgraded to `approve`; the
/// call site should increment the metric and emit a self-observability event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluateResult {
    pub decision: PolicyDecision,
    /// Set when the circuit breaker fired on this call.
    pub breaker_tripped: bool,
}

/// The full policy engine: the declarative [`Policy`], the global kill switch,
/// and the per-`(host, template)` circuit breaker. Shared via `Arc` in
/// `AppState`; [`Self::evaluate`] is the single decision entry point.
pub struct PolicyEngine {
    policy: Policy,
    /// Fleet-wide kill switch (`RAVN_REMEDIATION_AUTO`). When `false`, every
    /// decision is forced to `approve`.
    pub auto_enabled: bool,
    breaker: CircuitBreaker,
}

impl PolicyEngine {
    /// Build the engine from a policy and the auto/circuit-breaker settings.
    pub fn new(policy: Policy, auto_enabled: bool, max_auto: u32, window: Duration) -> Self {
        Self { policy, auto_enabled, breaker: CircuitBreaker::new(max_auto, window) }
    }

    /// Decide what to do with a matched remediation, applying every guardrail:
    /// kill switch → `dangerous`-never-auto → policy rule → circuit breaker.
    ///
    /// The returned [`EvaluateResult`] is final. `breaker_tripped` is `true`
    /// when the circuit breaker downgraded `auto` → `approve` on this call so
    /// the caller can emit a self-observability metric and Ravn event (#149).
    pub fn evaluate(
        &self,
        host: &str,
        tier: RiskTier,
        template_id: &str,
        now: DateTime<Utc>,
    ) -> EvaluateResult {
        // Kill switch forces everything to manual approval.
        if !self.auto_enabled {
            let decision = match self.policy.decide(host, tier) {
                PolicyDecision::Forbid => PolicyDecision::Forbid,
                _ => PolicyDecision::Approve,
            };
            return EvaluateResult { decision, breaker_tripped: false };
        }

        let mut decision = self.policy.decide(host, tier);

        // `dangerous` never auto-executes, regardless of the rule.
        if decision == PolicyDecision::Auto && tier == RiskTier::Dangerous {
            tracing::warn!(host, template = template_id, "policy auto for dangerous tier downgraded to approve");
            decision = PolicyDecision::Approve;
        }

        // Circuit breaker: too many recent auto-executions for this
        // (host, template) → escalate to a human instead.
        if decision == PolicyDecision::Auto {
            if self.breaker.tripped(host, template_id, now) {
                tracing::warn!(host, template = template_id, "circuit breaker tripped; auto downgraded to approve");
                return EvaluateResult { decision: PolicyDecision::Approve, breaker_tripped: true };
            }
            self.breaker.record(host, template_id, now);
        }

        EvaluateResult { decision, breaker_tripped: false }
    }
}

/// `(host, template_id)` → recent auto-execution timestamps within the window.
type HitLog = HashMap<(String, String), Vec<DateTime<Utc>>>;

/// A per-`(host, template)` sliding-window rate limiter. It permits at most
/// `max_auto` auto-executions within `window`; beyond that it trips, downgrading
/// further auto decisions to approval until older executions age out.
struct CircuitBreaker {
    max_auto: u32,
    window: Duration,
    hits: Mutex<HitLog>,
}

impl CircuitBreaker {
    fn new(max_auto: u32, window: Duration) -> Self {
        Self { max_auto, window, hits: Mutex::new(HashMap::new()) }
    }

    /// Whether a further auto-execution for `(host, template)` would exceed the
    /// cap. Prunes timestamps that have aged out of the window as a side effect.
    fn tripped(&self, host: &str, template_id: &str, now: DateTime<Utc>) -> bool {
        let cutoff = now - chrono::Duration::from_std(self.window).unwrap_or_else(|_| chrono::Duration::zero());
        let mut hits = self.hits.lock().expect("circuit breaker poisoned");
        let entry = hits.entry((host.to_string(), template_id.to_string())).or_default();
        entry.retain(|t| *t > cutoff);
        entry.len() as u32 >= self.max_auto
    }

    /// Record an auto-execution for `(host, template)` at `now`.
    fn record(&self, host: &str, template_id: &str, now: DateTime<Utc>) {
        let mut hits = self.hits.lock().expect("circuit breaker poisoned");
        hits.entry((host.to_string(), template_id.to_string())).or_default().push(now);
    }
}

/// Minimal glob match supporting `*` (any run, including empty) and `?` (one
/// char) — enough for host scopes like `web-*` or `*.prod` without pulling in a
/// glob dependency. Matching is over bytes; hostnames are ASCII.
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    // Classic two-pointer wildcard match with backtracking on `*`.
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut star_ti) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(toml_src: &str, auto_enabled: bool, max_auto: u32) -> PolicyEngine {
        let policy: Policy = toml::from_str(toml_src).expect("parse policy");
        PolicyEngine::new(policy, auto_enabled, max_auto, Duration::from_secs(60))
    }

    /// Shorthand: extract just the decision from the result.
    fn decide(eng: &PolicyEngine, host: &str, tier: RiskTier, tmpl: &str, now: DateTime<Utc>) -> PolicyDecision {
        eng.evaluate(host, tier, tmpl, now).decision
    }

    #[test]
    fn example_policy_file_loads_and_behaves() {
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../policy/example.toml"));
        let policy = Policy::load_file(path).expect("load example policy");
        let eng = PolicyEngine::new(policy, true, 5, Duration::from_secs(60));
        let now = Utc::now();
        // web-* safe → auto; db-* dangerous → forbid; guarded → approve.
        assert_eq!(decide(&eng, "web-1", RiskTier::Safe, "t", now), PolicyDecision::Auto);
        assert_eq!(decide(&eng, "db-1", RiskTier::Dangerous, "t", now), PolicyDecision::Forbid);
        assert_eq!(decide(&eng, "any-host", RiskTier::Guarded, "t", now), PolicyDecision::Approve);
        // A missing file is the default-deny policy: everything approves.
        let missing = Policy::load_file(std::path::Path::new("/no/such/policy.toml")).unwrap();
        let eng = PolicyEngine::new(missing, true, 5, Duration::from_secs(60));
        assert_eq!(decide(&eng, "web-1", RiskTier::Safe, "t", now), PolicyDecision::Approve);
    }

    #[test]
    fn glob_matches_host_scopes() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("web-*", "web-1"));
        assert!(glob_match("*.prod", "db.prod"));
        assert!(glob_match("host-?", "host-1"));
        assert!(!glob_match("web-*", "db-1"));
        assert!(!glob_match("host-?", "host-12"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exacts"));
    }

    #[test]
    fn default_deny_with_no_rules_is_approve() {
        let eng = engine("", true, 5);
        assert_eq!(
            decide(&eng, "host-1", RiskTier::Safe, "t", Utc::now()),
            PolicyDecision::Approve
        );
    }

    #[test]
    fn auto_rule_auto_executes() {
        let eng = engine(
            r#"[[rule]]
               tier = "safe"
               decision = "auto""#,
            true,
            5,
        );
        assert_eq!(
            decide(&eng, "host-1", RiskTier::Safe, "t", Utc::now()),
            PolicyDecision::Auto
        );
    }

    #[test]
    fn forbid_rule_produces_forbid() {
        let eng = engine(
            r#"[[rule]]
               tier = "guarded"
               decision = "forbid""#,
            true,
            5,
        );
        assert_eq!(
            decide(&eng, "host-1", RiskTier::Guarded, "t", Utc::now()),
            PolicyDecision::Forbid
        );
    }

    #[test]
    fn host_glob_scopes_a_rule() {
        let eng = engine(
            r#"[[rule]]
               host = "web-*"
               tier = "safe"
               decision = "auto""#,
            true,
            5,
        );
        let now = Utc::now();
        assert_eq!(decide(&eng, "web-1", RiskTier::Safe, "t", now), PolicyDecision::Auto);
        // Different host falls through to default-deny (approve).
        assert_eq!(decide(&eng, "db-1", RiskTier::Safe, "t", now), PolicyDecision::Approve);
    }

    #[test]
    fn first_matching_rule_wins() {
        let eng = engine(
            r#"[[rule]]
               host = "web-1"
               tier = "safe"
               decision = "forbid"
               [[rule]]
               tier = "safe"
               decision = "auto""#,
            true,
            5,
        );
        let now = Utc::now();
        assert_eq!(decide(&eng, "web-1", RiskTier::Safe, "t", now), PolicyDecision::Forbid);
        assert_eq!(decide(&eng, "web-2", RiskTier::Safe, "t", now), PolicyDecision::Auto);
    }

    #[test]
    fn dangerous_never_auto_executes() {
        let eng = engine(
            r#"[[rule]]
               tier = "dangerous"
               decision = "auto""#,
            true,
            5,
        );
        // Even an explicit `auto` rule is downgraded for the dangerous tier.
        assert_eq!(
            decide(&eng, "host-1", RiskTier::Dangerous, "t", Utc::now()),
            PolicyDecision::Approve
        );
    }

    #[test]
    fn kill_switch_forces_approve_but_keeps_forbid() {
        let eng = engine(
            r#"[[rule]]
               tier = "safe"
               decision = "auto"
               [[rule]]
               tier = "guarded"
               decision = "forbid""#,
            false, // kill switch: auto disabled
            5,
        );
        let now = Utc::now();
        // `auto` is forced to `approve` …
        assert_eq!(decide(&eng, "host-1", RiskTier::Safe, "t", now), PolicyDecision::Approve);
        // … but `forbid` still forbids (kill switch never widens execution).
        assert_eq!(decide(&eng, "host-1", RiskTier::Guarded, "t", now), PolicyDecision::Forbid);
    }

    #[test]
    fn circuit_breaker_downgrades_after_cap() {
        let eng = engine(
            r#"[[rule]]
               tier = "safe"
               decision = "auto""#,
            true,
            2, // cap of 2 auto-executions in the window
        );
        let now = Utc::now();
        // First two auto-executions pass.
        assert_eq!(decide(&eng, "host-1", RiskTier::Safe, "t", now), PolicyDecision::Auto);
        assert_eq!(decide(&eng, "host-1", RiskTier::Safe, "t", now), PolicyDecision::Auto);
        // The third trips the breaker and escalates to a human.
        let result = eng.evaluate("host-1", RiskTier::Safe, "t", now);
        assert_eq!(result.decision, PolicyDecision::Approve);
        assert!(result.breaker_tripped, "breaker_tripped must be set on the third call");
        // A different (host, template) pair has its own independent budget.
        assert_eq!(decide(&eng, "host-2", RiskTier::Safe, "t", now), PolicyDecision::Auto);
        assert_eq!(decide(&eng, "host-1", RiskTier::Safe, "other", now), PolicyDecision::Auto);
    }

    #[test]
    fn circuit_breaker_recovers_after_window() {
        let policy: Policy = toml::from_str(
            r#"[[rule]]
               tier = "safe"
               decision = "auto""#,
        )
        .unwrap();
        // A tiny 10ms window so aged-out hits clear quickly.
        let eng = PolicyEngine::new(policy, true, 1, Duration::from_millis(10));
        let t0 = Utc::now();
        assert_eq!(decide(&eng, "host-1", RiskTier::Safe, "t", t0), PolicyDecision::Auto);
        // Immediately again → tripped.
        let result = eng.evaluate("host-1", RiskTier::Safe, "t", t0);
        assert_eq!(result.decision, PolicyDecision::Approve);
        assert!(result.breaker_tripped);
        // After the window elapses, the old hit ages out and auto resumes.
        let later = t0 + chrono::Duration::milliseconds(50);
        assert_eq!(decide(&eng, "host-1", RiskTier::Safe, "t", later), PolicyDecision::Auto);
    }
}
