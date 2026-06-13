//! Comparison-table and site-page rendering for the benchmark (#157).
//!
//! The runner produces one [`Row`] per model; this module renders them as a
//! deterministic markdown comparison table (best overall first) plus a fuller
//! site page suitable for publishing as build-in-public content.

use std::fmt::Write;

use crate::fixtures::{Category, Fixture};

/// One model's aggregated benchmark result across the fixture corpus.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub model: String,
    /// Results came from a recorded run, not live inference.
    pub recorded: bool,
    /// Where/how a recorded run was captured (CPU, quant, server build); empty
    /// for live runs. Surfaced in the site page so the numbers are reproducible.
    pub captured_on: String,
    /// Mean faithfulness across the corpus (`0.0..=1.0`).
    pub faithfulness: f64,
    /// Mean actionability across the corpus (`0.0..=1.0`).
    pub actionability: f64,
    /// Mean overall quality across the corpus (`0.0..=1.0`).
    pub overall: f64,
    /// Mean end-to-end latency per event (ms).
    pub latency_ms: f64,
    /// Mean prompt-processing throughput (tokens/sec), if known.
    pub prompt_tps: Option<f64>,
    /// Mean generation throughput (tokens/sec), if known.
    pub gen_tps: Option<f64>,
    /// Resident memory of the model while serving, if known (MiB).
    pub mem_mb: Option<f64>,
    /// Number of fixtures the model was scored on.
    pub fixtures: usize,
    /// Mean overall quality per category, for the rollup table.
    pub per_category: Vec<(Category, f64)>,
}

fn fmt_opt(v: Option<f64>, prec: usize) -> String {
    match v {
        Some(x) => format!("{x:.*}", prec),
        None => "n/a".to_string(),
    }
}

/// Sort rows deterministically: overall quality descending, then model name.
fn sorted(rows: &[Row]) -> Vec<&Row> {
    let mut rows: Vec<&Row> = rows.iter().collect();
    rows.sort_by(|a, b| {
        b.overall
            .partial_cmp(&a.overall)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });
    rows
}

/// Render the headline comparison table, best overall first.
pub fn render_table(rows: &[Row]) -> String {
    let mut out = String::new();
    out.push_str(
        "| Model | Overall | Faithful | Actionable | Latency ms | Prompt tok/s | Gen tok/s | Memory MiB | Source |\n",
    );
    out.push_str("|---|--:|--:|--:|--:|--:|--:|--:|:--|\n");
    for r in sorted(rows) {
        let source = if r.recorded { "recorded" } else { "live" };
        let _ = writeln!(
            out,
            "| {} | {:.2} | {:.2} | {:.2} | {:.0} | {} | {} | {} | {} |",
            r.model,
            r.overall,
            r.faithfulness,
            r.actionability,
            r.latency_ms,
            fmt_opt(r.prompt_tps, 0),
            fmt_opt(r.gen_tps, 1),
            fmt_opt(r.mem_mb, 0),
            source,
        );
    }
    out
}

/// Render the per-category overall-quality rollup.
pub fn render_category_table(rows: &[Row]) -> String {
    // Collect the category set in a stable order from the first row.
    let categories: Vec<Category> = rows
        .first()
        .map(|r| r.per_category.iter().map(|(c, _)| *c).collect())
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("| Model |");
    for c in &categories {
        let _ = write!(out, " {} |", c.label());
    }
    out.push('\n');
    out.push_str("|---|");
    for _ in &categories {
        out.push_str("--:|");
    }
    out.push('\n');

    for r in sorted(rows) {
        let _ = write!(out, "| {} |", r.model);
        for c in &categories {
            let v = r.per_category.iter().find(|(cc, _)| cc == c).map(|(_, v)| *v);
            let _ = write!(out, " {} |", fmt_opt(v, 2));
        }
        out.push('\n');
    }
    out
}

/// Render the full markdown site page: intro, headline table, category rollup,
/// methodology, provenance of recorded runs, and the reference-answer appendix.
pub fn render_site_page(rows: &[Row], corpus: &[Fixture]) -> String {
    let any_live = rows.iter().any(|r| !r.recorded);
    let mut out = String::new();

    out.push_str("# Can a sub-2B CPU model handle the explanation last-mile?\n\n");
    out.push_str(
        "Ravn's detection is deterministic — taps fire the alarm. The model only writes the \
         plain-language explanation and suggests one check. This page scores small, CPU-friendly \
         models on exactly that job, over a fixed corpus of real-shaped events.\n\n",
    );
    let _ = writeln!(
        out,
        "**Corpus:** {} events across failed units, OOM kills, auth anomalies, and \
         config drift. **Scoring:** deterministic, model-free (no LLM-as-judge).\n",
        corpus.len()
    );

    out.push_str("## Headline results\n\n");
    out.push_str(&render_table(rows));
    out.push('\n');

    out.push_str("## By category (overall quality)\n\n");
    out.push_str(&render_category_table(rows));
    out.push('\n');

    out.push_str("## Methodology\n\n");
    out.push_str(
        "- **Faithfulness** = grounded in the event's salient facts AND free of invented facts. \
         Each fixture ships hallucination traps (causes *not* in the event); naming one costs \
         points. This is the metric that catches fluent-but-wrong tiny models.\n\
         - **Actionability** = a concrete `suggested_check` was offered AND it targets the right \
         tool/subject (a generic \"please investigate\" scores low).\n\
         - **Overall** = 0.6·faithfulness + 0.3·actionability + 0.1·length-sanity.\n\
         - **Latency / memory** are measured against a live `llama-server` on CPU.\n\n",
    );

    // Provenance for recorded runs so the numbers are reproducible.
    let provenance: Vec<&Row> =
        rows.iter().filter(|r| r.recorded && !r.captured_on.is_empty()).collect();
    if !provenance.is_empty() {
        out.push_str("### Recorded-run provenance\n\n");
        for r in &provenance {
            let _ = writeln!(out, "- **{}** — {}", r.model, r.captured_on);
        }
        out.push('\n');
    }

    if !any_live {
        out.push_str(
            "> **Note:** every row on this page is a *recorded* run — captured model output \
             replayed through the live scoring harness. Re-run `ravn-eval --endpoint <url> \
             --models ...` against a `llama-server` to produce live numbers. The scores, table, \
             and methodology are identical; only the model responses are pre-captured.\n\n",
        );
    }

    // Reference appendix: the gold-standard answers the corpus is scored
    // against, so readers can judge the rubric and the models for themselves.
    out.push_str("## Reference answers\n\n");
    out.push_str(
        "The faithful, human-authored explanation and ideal check each fixture is scored \
         against:\n\n",
    );
    for f in corpus {
        let _ = writeln!(
            out,
            "### {} — {}\n\n{}\n",
            f.name,
            f.reference.category.label(),
            f.reference.reference_explanation,
        );
        if let Some(check) = &f.reference.expected_check {
            let _ = writeln!(out, "Ideal check: `{check}`\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(model: &str, overall: f64, recorded: bool) -> Row {
        Row {
            model: model.into(),
            recorded,
            captured_on: if recorded { "test CPU".into() } else { String::new() },
            faithfulness: overall,
            actionability: overall,
            overall,
            latency_ms: 900.0,
            prompt_tps: Some(200.0),
            gen_tps: Some(18.5),
            mem_mb: Some(1400.0),
            fixtures: 6,
            per_category: vec![
                (Category::FailedUnit, overall),
                (Category::OomKill, overall),
            ],
        }
    }

    #[test]
    fn table_sorts_by_overall_desc() {
        let table = render_table(&[row("worse", 0.40, true), row("better", 0.90, true)]);
        let better = table.find("better").unwrap();
        let worse = table.find("worse").unwrap();
        assert!(better < worse, "higher overall must appear first");
    }

    #[test]
    fn table_marks_source_and_handles_missing_memory() {
        let mut r = row("m", 0.5, false);
        r.mem_mb = None;
        let table = render_table(&[r]);
        assert!(table.contains("| Model | Overall |"));
        assert!(table.contains("| live |"));
        assert!(table.contains("| n/a |"));
    }

    #[test]
    fn category_table_lists_each_category() {
        let table = render_category_table(&[row("m", 0.7, true)]);
        assert!(table.contains("failed unit"));
        assert!(table.contains("OOM kill"));
    }

    #[test]
    fn site_page_notes_recorded_runs() {
        use crate::fixtures::{default_corpus_dir, load_corpus};
        let corpus = load_corpus(&default_corpus_dir()).expect("corpus");
        let page = render_site_page(&[row("m", 0.7, true)], &corpus);
        assert!(page.contains("recorded"));
        assert!(page.contains("## Headline results"));
        assert!(page.contains("## Methodology"));
        assert!(page.contains("## Reference answers"));
        assert!(page.contains("### Recorded-run provenance"));
        // The provenance line uses captured_on.
        assert!(page.contains("test CPU"));
    }
}
