//! Fixture corpus loading: real `ravn_core::Event`s paired with hand-authored
//! reference explanations (#157).
//!
//! Each case in `fixtures/corpus/` is a pair sharing a base name:
//!
//! - `<name>.event.json` — a serialized [`ravn_core::Event`], a sanitised but
//!   realistic detection event from one of the four categories the issue calls
//!   out (failed units, OOM kills, auth anomalies, config drift).
//! - `<name>.reference.json` — the [`Reference`] gold-standard explanation plus
//!   the faithfulness anchors and actionability expectations used to score a
//!   candidate model's answer deterministically.
//!
//! Loading is hermetic and unit-tested; no model is required to validate the
//! corpus, which is what makes the harness `nix flake check`-able offline.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ravn_core::Event;
use serde::Deserialize;

/// The eval category a fixture belongs to. Drives the per-category rollup in
/// the report and maps directly onto the four buckets named in #157.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// A systemd/Kubernetes unit or workload that failed to stay running.
    FailedUnit,
    /// A process or container killed by the OOM killer (host or pod).
    OomKill,
    /// Suspicious authentication activity (brute force, unexpected root login).
    AuthAnomaly,
    /// A watched configuration file whose contents drifted.
    ConfigDrift,
}

impl Category {
    /// Stable, human-facing label for the report.
    pub fn label(self) -> &'static str {
        match self {
            Category::FailedUnit => "failed unit",
            Category::OomKill => "OOM kill",
            Category::AuthAnomaly => "auth anomaly",
            Category::ConfigDrift => "config drift",
        }
    }
}

/// The hand-authored reference for one fixture: what a good explanation looks
/// like, and the deterministic anchors used to score a candidate against it.
#[derive(Debug, Clone, Deserialize)]
pub struct Reference {
    /// Which of the four eval categories this fixture exercises.
    pub category: Category,
    /// The gold-standard explanation, written by a human operator. Shown in the
    /// report for qualitative comparison; not used numerically.
    pub reference_explanation: String,
    /// Salient facts a faithful explanation must reference. These are the
    /// faithfulness ground-truth: the union with the payload-derived salient
    /// tokens forms the grounding set.
    #[serde(default)]
    pub must_mention: Vec<String>,
    /// Hallucination traps: facts NOT present in the event that a model must not
    /// invent. Any of these appearing in the answer is penalised as an invented
    /// fact — this is how we measure "no invented facts".
    #[serde(default)]
    pub forbidden: Vec<String>,
    /// The ideal command/check, shown in the report.
    #[serde(default)]
    pub expected_check: Option<String>,
    /// Tokens an acceptable `suggested_check` should contain (e.g. the tool and
    /// the subject). Drives the actionability score.
    #[serde(default)]
    pub check_keywords: Vec<String>,
}

/// A loaded fixture: its name, the detection event, and the reference.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub name: String,
    pub event: Event,
    pub reference: Reference,
}

/// The default corpus directory shipped with the crate.
pub fn default_corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus")
}

/// Load every `<name>.event.json` + `<name>.reference.json` pair in `dir`,
/// sorted by name for deterministic ordering.
pub fn load_corpus(dir: &Path) -> Result<Vec<Fixture>> {
    let mut out = Vec::new();
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading corpus dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(".event.json") else {
            continue;
        };
        let event_json = std::fs::read_to_string(entry.path())
            .with_context(|| format!("reading {file_name}"))?;
        let event: Event = serde_json::from_str(&event_json)
            .with_context(|| format!("parsing {file_name} as Event"))?;

        let ref_path = dir.join(format!("{name}.reference.json"));
        let ref_json = std::fs::read_to_string(&ref_path)
            .with_context(|| format!("reading reference for {name} at {}", ref_path.display()))?;
        let reference: Reference = serde_json::from_str(&ref_json)
            .with_context(|| format!("parsing {name}.reference.json"))?;

        out.push(Fixture { name: name.to_string(), event, reference });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_all_corpus_pairs() {
        let corpus = load_corpus(&default_corpus_dir()).expect("load corpus");
        // #157 requires the four categories; we ship at least one per category.
        assert!(corpus.len() >= 5, "expected >=5 fixtures, got {}", corpus.len());
        assert!(corpus.iter().any(|f| f.name == "failed_unit_nginx"));

        for cat in [
            Category::FailedUnit,
            Category::OomKill,
            Category::AuthAnomaly,
            Category::ConfigDrift,
        ] {
            assert!(
                corpus.iter().any(|f| f.reference.category == cat),
                "no fixture for category {:?}",
                cat
            );
        }
    }

    #[test]
    fn references_are_well_formed() {
        for f in load_corpus(&default_corpus_dir()).expect("load corpus") {
            assert!(
                !f.reference.reference_explanation.trim().is_empty(),
                "{} has empty reference explanation",
                f.name
            );
            assert!(
                !f.reference.must_mention.is_empty(),
                "{} has no must_mention anchors",
                f.name
            );
            // The event must round-trip back to the same source it claims.
            let _ = f.event.source();
        }
    }
}
