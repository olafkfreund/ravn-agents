//! Deterministic quality rubric for model explanations (#157).
//!
//! Scoring is intentionally model-free and reproducible — no LLM-as-judge — so
//! the same fixture + the same model output always yields the same score, and
//! the harness can gate `nix flake check`. It scores the two properties the
//! issue cares about:
//!
//! - **faithfulness** — the explanation is grounded in the event's salient
//!   facts AND does not invent facts that aren't in the event. The second half
//!   is the one that matters most for a tiny model: fluent hallucination is the
//!   failure mode we most want to catch.
//! - **actionability** — the model offered a concrete check, and that check
//!   targets the right tool/subject rather than a generic "investigate".
//!
//! It is a *guardrail* metric to compare candidate models on a fixed corpus,
//! not a substitute for human judgement. Latency/RAM are measured separately by
//! the runner.

use ravn_core::{Event, Payload};

use crate::fixtures::Reference;

/// A breakdown of an explanation's quality. Every component is in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    /// The explanation text is non-empty.
    pub non_empty: bool,
    /// Fraction of the salient facts (payload-derived + `must_mention`) the
    /// explanation references. Higher is better.
    pub grounded: f64,
    /// `1.0` if the explanation invented no forbidden facts, scaled down for
    /// each hallucination trap it tripped. Higher is better.
    pub no_hallucination: f64,
    /// A non-empty `suggested_check` was provided.
    pub has_check: bool,
    /// Fraction of the reference's `check_keywords` the suggested check hits —
    /// rewards a *specific* check over a generic one.
    pub check_specificity: f64,
    /// Explanation length is within a sensible range.
    pub length_ok: bool,
}

impl Score {
    /// Faithfulness sub-score in `0.0..=1.0`: grounded in the facts and free of
    /// invented ones. Grounding and not-hallucinating are weighted equally —
    /// a model that omits facts and one that invents them are both unfaithful.
    pub fn faithfulness(&self) -> f64 {
        if !self.non_empty {
            return 0.0;
        }
        0.5 * self.grounded + 0.5 * self.no_hallucination
    }

    /// Actionability sub-score in `0.0..=1.0`: offered a check, and the check is
    /// specific to the event. Presence is necessary; specificity is the reward.
    pub fn actionability(&self) -> f64 {
        if !self.has_check {
            return 0.0;
        }
        0.5 + 0.5 * self.check_specificity
    }

    /// Weighted overall score in `0.0..=1.0`.
    ///
    /// Faithfulness dominates (0.6) — for the explanation last-mile, being right
    /// matters more than being actionable, and far more than being verbose.
    pub fn overall(&self) -> f64 {
        if !self.non_empty {
            return 0.0;
        }
        0.6 * self.faithfulness() + 0.3 * self.actionability() + 0.1 * f64::from(self.length_ok)
    }
}

/// Salient, deterministic tokens an explanation of `event` ought to reference.
///
/// Drawn from the structured payload (never the freeform message), so the set
/// is stable and unambiguous.
pub fn salient_tokens(event: &Event) -> Vec<String> {
    let mut tokens = vec![event.host.clone()];
    match &event.payload {
        Payload::Journald(p) => {
            if let Some(unit) = &p.unit {
                tokens.push(unit.clone());
            }
        }
        Payload::FailedUnit(p) => {
            tokens.push(p.unit.clone());
            tokens.push(p.result.clone());
        }
        Payload::ConfigDrift(p) => {
            // The basename is what a human would name, e.g. `sshd_config`.
            let base = p.path.rsplit('/').next().unwrap_or(&p.path);
            tokens.push(base.to_string());
        }
        Payload::Auth(p) => {
            tokens.push(p.action.clone());
            if let Some(user) = &p.user {
                tokens.push(user.clone());
            }
            if let Some(addr) = &p.remote_addr {
                tokens.push(addr.clone());
            }
        }
        Payload::Update(p) => {
            tokens.push(p.mechanism.clone());
            if let Some(to) = &p.to {
                tokens.push(to.clone());
            }
        }
        Payload::KubeWorkload(p) => {
            tokens.push(p.namespace.clone());
            tokens.push(p.name.clone());
            tokens.push(p.reason.clone());
        }
        Payload::KubeNode(p) => {
            tokens.push(p.node.clone());
            tokens.push(p.condition.clone());
        }
    }
    tokens.retain(|t| !t.trim().is_empty());
    tokens
}

/// How many of `needles` appear (case-insensitively) in `haystack`.
///
/// `haystack` is expected to already be lowercased; `needles` are lowercased
/// here so callers can pass raw anchors.
fn hits(haystack: &str, needles: &[String]) -> usize {
    needles.iter().filter(|n| haystack.contains(&n.to_lowercase())).count()
}

/// Score an explanation (and optional suggested check) against an event and its
/// reference. Deterministic and side-effect free.
pub fn score(
    event: &Event,
    reference: &Reference,
    explanation: &str,
    suggested_check: Option<&str>,
) -> Score {
    let text = explanation.trim();
    let non_empty = !text.is_empty();

    let explanation_lc = explanation.to_lowercase();
    let check_lc = suggested_check.unwrap_or("").to_lowercase();
    let haystack = format!("{explanation_lc} {check_lc}");

    // Grounding: union of payload-derived tokens and the reference's
    // must_mention anchors, de-duplicated case-insensitively.
    let mut anchors = salient_tokens(event);
    anchors.extend(reference.must_mention.iter().cloned());
    let mut seen = std::collections::BTreeSet::new();
    anchors.retain(|t| seen.insert(t.to_lowercase()));

    let grounded = if anchors.is_empty() {
        1.0
    } else {
        hits(&haystack, &anchors) as f64 / anchors.len() as f64
    };

    // Hallucination: each forbidden fact that shows up costs an equal slice.
    // Only the explanation text is searched (a check naming a tool shouldn't be
    // penalised), and an empty explanation can't hallucinate — it's already
    // zeroed by `non_empty`.
    let no_hallucination = if reference.forbidden.is_empty() {
        1.0
    } else {
        let tripped = hits(&explanation_lc, &reference.forbidden);
        1.0 - (tripped as f64 / reference.forbidden.len() as f64)
    };

    let has_check = suggested_check.map(|c| !c.trim().is_empty()).unwrap_or(false);
    let check_specificity = if reference.check_keywords.is_empty() {
        f64::from(has_check)
    } else if has_check {
        hits(&check_lc, &reference.check_keywords) as f64 / reference.check_keywords.len() as f64
    } else {
        0.0
    };

    let length_ok = (20..=600).contains(&text.chars().count());

    Score {
        non_empty,
        grounded,
        no_hallucination,
        has_check,
        check_specificity,
        length_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Category, Reference};
    use chrono::Utc;
    use ravn_core::{AgentId, FailedUnitPayload, Severity};
    use uuid::Uuid;

    fn failed_unit_event() -> Event {
        let now = Utc::now();
        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::now_v7()),
            host: "web-01".into(),
            severity: Severity::Error,
            title: "unit failed: nginx.service".into(),
            category_hints: vec![],
            payload: Payload::FailedUnit(FailedUnitPayload {
                unit: "nginx.service".into(),
                result: "exit-code".into(),
                recent_log: vec![],
                ..Default::default()
            }),
        }
    }

    fn reference() -> Reference {
        Reference {
            category: Category::FailedUnit,
            reference_explanation: "nginx failed to bind port 80".into(),
            must_mention: vec!["80".into(), "bind".into()],
            forbidden: vec!["out of memory".into(), "disk full".into()],
            expected_check: Some("ss -ltnp sport = :80".into()),
            check_keywords: vec!["ss".into(), "80".into()],
        }
    }

    #[test]
    fn salient_tokens_come_from_payload() {
        let toks = salient_tokens(&failed_unit_event());
        assert!(toks.contains(&"web-01".to_string()));
        assert!(toks.contains(&"nginx.service".to_string()));
        assert!(toks.contains(&"exit-code".to_string()));
    }

    #[test]
    fn faithful_actionable_explanation_scores_high() {
        let s = score(
            &failed_unit_event(),
            &reference(),
            "nginx.service on web-01 failed with an exit-code result because it could not bind to port 80.",
            Some("ss -ltnp sport = :80"),
        );
        assert_eq!(s.grounded, 1.0, "all anchors present");
        assert_eq!(s.no_hallucination, 1.0, "no forbidden facts");
        assert!(s.has_check && (s.check_specificity - 1.0).abs() < 1e-9);
        assert!(s.faithfulness() > 0.99);
        assert!(s.actionability() > 0.99);
        assert!(s.overall() > 0.95);
    }

    #[test]
    fn hallucinated_fact_tanks_faithfulness() {
        // Fluent and grounded, but invents an out-of-memory cause not in the event.
        let s = score(
            &failed_unit_event(),
            &reference(),
            "nginx.service on web-01 failed with exit-code because the host ran out of memory while binding port 80.",
            Some("ss -ltnp sport = :80"),
        );
        assert_eq!(s.grounded, 1.0);
        assert!(s.no_hallucination < 1.0, "tripped an OOM trap");
        assert!(
            s.faithfulness() < 0.85,
            "invented fact must drag faithfulness down, got {}",
            s.faithfulness()
        );
    }

    #[test]
    fn generic_check_is_less_actionable_than_specific() {
        let specific = score(
            &failed_unit_event(),
            &reference(),
            "nginx.service on web-01 failed (exit-code); port 80 bind failed.",
            Some("ss -ltnp sport = :80"),
        );
        let generic = score(
            &failed_unit_event(),
            &reference(),
            "nginx.service on web-01 failed (exit-code); port 80 bind failed.",
            Some("please investigate the service"),
        );
        assert!(
            specific.actionability() > generic.actionability(),
            "a targeted check must beat a generic one"
        );
    }

    #[test]
    fn empty_explanation_scores_zero() {
        let s = score(&failed_unit_event(), &reference(), "   ", None);
        assert!(!s.non_empty);
        assert_eq!(s.overall(), 0.0);
        assert_eq!(s.faithfulness(), 0.0);
    }
}
