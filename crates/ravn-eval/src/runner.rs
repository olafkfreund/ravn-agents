//! The scoring loop: drive a [`ModelBackend`] over the corpus and aggregate
//! per-model [`Row`]s (#157).
//!
//! The loop is backend-agnostic — it doesn't care whether explanations come
//! from a live `llama-server` or a recorded run — so the same code produces the
//! reproducible CI table and the live CPU benchmark.

use std::collections::BTreeMap;

use anyhow::Result;
use ravn_agent::inference::parse_explanation;

use crate::backend::{Completion, ModelBackend, RecordedBackend};
use crate::fixtures::{Category, Fixture};
use crate::report::Row;
use crate::rubric::{self, Score};

/// Per-fixture scored outcome.
pub struct EventResult {
    pub name: String,
    pub category: Category,
    pub latency_ms: f64,
    pub prompt_tps: Option<f64>,
    pub gen_tps: Option<f64>,
    pub score: Score,
}

/// Score a single completion against a fixture.
pub fn score_completion(fixture: &Fixture, completion: &Completion) -> EventResult {
    let (text, check) = parse_explanation(&completion.content);
    let score = rubric::score(&fixture.event, &fixture.reference, &text, check.as_deref());
    EventResult {
        name: fixture.name.clone(),
        category: fixture.reference.category,
        latency_ms: completion.latency_ms,
        prompt_tps: completion.prompt_tps,
        gen_tps: completion.gen_tps,
        score,
    }
}

/// Mean of the reported (`Some`) values, or `None` if none reported.
fn mean_reported(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let reported: Vec<f64> = values.flatten().collect();
    if reported.is_empty() {
        None
    } else {
        Some(reported.iter().sum::<f64>() / reported.len() as f64)
    }
}

/// Aggregate per-fixture results into a model [`Row`], including the
/// per-category overall-quality rollup.
fn aggregate(
    model: &str,
    recorded: bool,
    captured_on: String,
    mem_mb: Option<f64>,
    results: &[EventResult],
) -> Row {
    let n = results.len().max(1) as f64;
    let mean = |f: &dyn Fn(&EventResult) -> f64| results.iter().map(f).sum::<f64>() / n;

    let gen_tps = mean_reported(results.iter().map(|r| r.gen_tps));
    let prompt_tps = mean_reported(results.iter().map(|r| r.prompt_tps));

    // Per-category overall mean, in a stable category order.
    let mut buckets: BTreeMap<&'static str, (Category, f64, usize)> = BTreeMap::new();
    for r in results {
        let entry = buckets.entry(r.category.label()).or_insert((r.category, 0.0, 0));
        entry.1 += r.score.overall();
        entry.2 += 1;
    }
    let per_category = buckets
        .values()
        .map(|(cat, sum, count)| (*cat, sum / *count as f64))
        .collect();

    Row {
        model: model.to_string(),
        recorded,
        captured_on,
        faithfulness: mean(&|r| r.score.faithfulness()),
        actionability: mean(&|r| r.score.actionability()),
        overall: mean(&|r| r.score.overall()),
        latency_ms: mean(&|r| r.latency_ms),
        prompt_tps,
        gen_tps,
        mem_mb,
        fixtures: results.len(),
        per_category,
    }
}

/// Benchmark a live backend across the whole corpus.
pub async fn bench_live(backend: &dyn ModelBackend, corpus: &[Fixture]) -> Result<Row> {
    let mut results = Vec::with_capacity(corpus.len());
    for fixture in corpus {
        let completion = backend.complete(&fixture.event).await?;
        let r = score_completion(fixture, &completion);
        println!(
            "  {:<28} overall={:.2}  faithful={:.2}  action={:.2}  {:.0}ms",
            r.name,
            r.score.overall(),
            r.score.faithfulness(),
            r.score.actionability(),
            r.latency_ms
        );
        results.push(r);
    }
    let mem_mb = backend.memory_mb().await;
    Ok(aggregate(
        backend.name(),
        backend.is_recorded(),
        backend.captured_on().to_string(),
        mem_mb,
        &results,
    ))
}

/// Benchmark a recorded backend, replaying responses keyed by fixture name.
pub async fn bench_recorded(backend: &RecordedBackend, corpus: &[Fixture]) -> Result<Row> {
    let mut results = Vec::with_capacity(corpus.len());
    for fixture in corpus {
        let completion = backend.complete_named(&fixture.name)?;
        let r = score_completion(fixture, &completion);
        println!(
            "  {:<28} overall={:.2}  faithful={:.2}  action={:.2}",
            r.name,
            r.score.overall(),
            r.score.faithfulness(),
            r.score.actionability()
        );
        results.push(r);
    }
    let mem_mb = backend.memory_mb().await;
    Ok(aggregate(backend.name(), true, backend.captured_on().to_string(), mem_mb, &results))
}

/// Replay every committed recorded run under `recordings_root` against `corpus`,
/// returning one [`Row`] per model sorted by directory name. This is the
/// reproducible default path — what CI and the golden test exercise.
pub async fn run_all_recorded(
    recordings_root: &std::path::Path,
    corpus: &[Fixture],
) -> Result<Vec<Row>> {
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(recordings_root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").exists())
        .collect();
    dirs.sort();

    let mut rows = Vec::new();
    for dir in &dirs {
        let backend = RecordedBackend::load(dir)?;
        rows.push(bench_recorded(&backend, corpus).await?);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::RecordedBackend;
    use crate::fixtures::{default_corpus_dir, load_corpus};

    #[tokio::test]
    async fn recorded_run_produces_scored_rows_for_three_models() {
        let corpus = load_corpus(&default_corpus_dir()).expect("corpus");
        let root = RecordedBackend::recordings_dir();

        let mut rows = Vec::new();
        for model in ["qwen3-1.7b", "qwen2.5-3b", "phi-3.5-mini"] {
            let backend = RecordedBackend::load(&root.join(model)).expect("load recording");
            let row = bench_recorded(&backend, &corpus).await.expect("bench");
            assert_eq!(row.fixtures, corpus.len(), "scored every fixture");
            assert!(row.recorded);
            assert!((0.0..=1.0).contains(&row.overall));
            assert!(!row.per_category.is_empty());
            rows.push(row);
        }

        assert_eq!(rows.len(), 3, "#157 needs >=3 scored models");
        // A capable model should beat the weak baseline if present.
        let qwen3 = rows.iter().find(|r| r.model.contains("qwen3")).unwrap();
        assert!(qwen3.overall > 0.5, "qwen3 should score reasonably, got {}", qwen3.overall);
    }

    #[tokio::test]
    async fn weak_baseline_ranks_below_capable_models() {
        // The harness must *discriminate*: the rubric is only useful if a
        // fluent-but-ungrounded model scores clearly worse. This is the core
        // evidence for #157's claim, in test form.
        let corpus = load_corpus(&default_corpus_dir()).expect("corpus");
        let rows = run_all_recorded(&RecordedBackend::recordings_dir(), &corpus)
            .await
            .expect("recorded run");

        let weak = rows.iter().find(|r| r.model.contains("tinyllama")).unwrap();
        let best = rows.iter().map(|r| r.overall).fold(0.0_f64, f64::max);
        assert!(
            weak.overall + 0.25 < best,
            "weak baseline ({:.2}) must rank well below the best model ({:.2})",
            weak.overall,
            best
        );
        // And the weak model must trip hallucination traps (low faithfulness).
        assert!(weak.faithfulness < 0.7, "weak faithfulness was {:.2}", weak.faithfulness);
    }

    /// Golden regression for the published results table + site page.
    ///
    /// Because scoring is deterministic and the recordings are committed, the
    /// rendered page is reproducible byte-for-byte. This test pins it so a
    /// rubric, corpus, or recording change shows up as a reviewable diff to the
    /// published page — and `nix flake check` fails if `RESULTS.md` is stale.
    ///
    /// Re-bless after an intended change with `RAVN_BLESS=1 cargo test -p ravn-eval`.
    #[tokio::test]
    async fn results_page_matches_golden() {
        use crate::report::render_site_page;
        use std::path::Path;

        let corpus = load_corpus(&default_corpus_dir()).expect("corpus");
        let rows = run_all_recorded(&RecordedBackend::recordings_dir(), &corpus)
            .await
            .expect("recorded run");
        let rendered = render_site_page(&rows, &corpus);

        let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("RESULTS.md");
        if std::env::var_os("RAVN_BLESS").is_some() {
            std::fs::write(&golden, &rendered).expect("bless RESULTS.md");
            return;
        }
        let expected = std::fs::read_to_string(&golden)
            .expect("missing RESULTS.md; run with RAVN_BLESS=1 to generate it");
        assert_eq!(
            rendered, expected,
            "RESULTS.md drifted from the harness output; re-bless with RAVN_BLESS=1"
        );
    }
}
