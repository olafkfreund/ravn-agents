//! Comparison-table rendering for the benchmark (#38).

/// One model's aggregated benchmark result across the fixture set.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub model: String,
    /// Mean prompt-processing throughput (tokens/sec).
    pub prompt_tps: f64,
    /// Mean generation throughput (tokens/sec).
    pub gen_tps: f64,
    /// Mean end-to-end latency per event (ms).
    pub latency_ms: f64,
    /// Resident memory of the server while serving, if known (MiB).
    pub mem_mb: Option<f64>,
    /// Mean quality score across the fixture set (`0.0..=1.0`).
    pub quality: f64,
    /// Number of fixtures the model was scored on.
    pub fixtures: usize,
}

/// Render a markdown comparison table, best quality first.
///
/// Sorting is deterministic: quality descending, then model name ascending to
/// break ties stably.
pub fn render_table(rows: &[Row]) -> String {
    let mut rows: Vec<&Row> = rows.iter().collect();
    rows.sort_by(|a, b| {
        b.quality
            .partial_cmp(&a.quality)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });

    let mut out = String::new();
    out.push_str("| Model | Quality | Prompt tok/s | Gen tok/s | Latency ms | Memory MiB | Fixtures |\n");
    out.push_str("|---|--:|--:|--:|--:|--:|--:|\n");
    for r in rows {
        let mem = match r.mem_mb {
            Some(m) => format!("{m:.0}"),
            None => "n/a".to_string(),
        };
        out.push_str(&format!(
            "| {} | {:.2} | {:.1} | {:.1} | {:.0} | {} | {} |\n",
            r.model, r.quality, r.prompt_tps, r.gen_tps, r.latency_ms, mem, r.fixtures,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(model: &str, quality: f64) -> Row {
        Row {
            model: model.into(),
            prompt_tps: 120.0,
            gen_tps: 18.5,
            latency_ms: 900.0,
            mem_mb: Some(512.0),
            quality,
            fixtures: 5,
        }
    }

    #[test]
    fn table_sorts_by_quality_desc() {
        let table = render_table(&[row("worse", 0.40), row("better", 0.90)]);
        let better = table.find("better").unwrap();
        let worse = table.find("worse").unwrap();
        assert!(better < worse, "higher quality must appear first");
    }

    #[test]
    fn table_has_header_and_handles_missing_memory() {
        let mut r = row("m", 0.5);
        r.mem_mb = None;
        let table = render_table(&[r]);
        assert!(table.contains("| Model | Quality |"));
        assert!(table.contains("| n/a |"));
    }
}
