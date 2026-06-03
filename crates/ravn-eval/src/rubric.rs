//! Deterministic quality rubric for model explanations (#38).
//!
//! Scoring is intentionally model-free and reproducible: it rewards an
//! explanation that (a) is non-empty, (b) is grounded in the salient facts of
//! the event rather than hallucinated, (c) offers a concrete check, and (d) is
//! a reasonable length. It is a *guardrail* metric to compare candidate models
//! on the fixture set — not a substitute for human judgement.

use ravn_core::{Event, Payload};

/// A breakdown of an explanation's quality, each component in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    /// The explanation text is non-empty.
    pub non_empty: bool,
    /// Fraction of the event's salient tokens referenced by the explanation.
    pub grounded: f64,
    /// A non-empty `suggested_check` was provided.
    pub has_check: bool,
    /// Explanation length is within a sensible range.
    pub length_ok: bool,
}

impl Score {
    /// Weighted overall score in `0.0..=1.0`.
    ///
    /// Grounding dominates (0.5) — a fluent but ungrounded explanation is the
    /// failure mode we most want to penalise.
    pub fn overall(&self) -> f64 {
        if !self.non_empty {
            return 0.0;
        }
        0.5 * self.grounded
            + 0.2 * f64::from(self.has_check)
            + 0.2 * f64::from(self.length_ok)
            + 0.1
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
    }
    tokens.retain(|t| !t.trim().is_empty());
    tokens
}

/// Score an explanation (and optional suggested check) against an event.
pub fn score(event: &Event, explanation: &str, suggested_check: Option<&str>) -> Score {
    let text = explanation.trim();
    let non_empty = !text.is_empty();

    let haystack = format!("{} {}", explanation, suggested_check.unwrap_or("")).to_lowercase();
    let tokens = salient_tokens(event);
    let grounded = if tokens.is_empty() {
        1.0
    } else {
        let hits = tokens
            .iter()
            .filter(|t| haystack.contains(&t.to_lowercase()))
            .count();
        hits as f64 / tokens.len() as f64
    };

    let has_check = suggested_check.map(|c| !c.trim().is_empty()).unwrap_or(false);
    let length_ok = (20..=600).contains(&text.chars().count());

    Score { non_empty, grounded, has_check, length_ok }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn salient_tokens_come_from_payload() {
        let toks = salient_tokens(&failed_unit_event());
        assert!(toks.contains(&"web-01".to_string()));
        assert!(toks.contains(&"nginx.service".to_string()));
        assert!(toks.contains(&"exit-code".to_string()));
    }

    #[test]
    fn grounded_explanation_scores_high() {
        let s = score(
            &failed_unit_event(),
            "nginx.service on web-01 failed with an exit-code result because the port was busy.",
            Some("systemctl status nginx"),
        );
        assert!(s.non_empty && s.has_check && s.length_ok);
        assert_eq!(s.grounded, 1.0);
        assert!(s.overall() > 0.9);
    }

    #[test]
    fn ungrounded_explanation_scores_low() {
        let s = score(
            &failed_unit_event(),
            "Something went wrong somewhere; please investigate the situation carefully.",
            None,
        );
        assert_eq!(s.grounded, 0.0);
        assert!(!s.has_check);
        // non_empty + length_ok only.
        assert!(s.overall() < 0.4);
    }

    #[test]
    fn empty_explanation_scores_zero() {
        let s = score(&failed_unit_event(), "   ", None);
        assert!(!s.non_empty);
        assert_eq!(s.overall(), 0.0);
    }
}
