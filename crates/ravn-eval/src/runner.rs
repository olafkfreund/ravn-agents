//! Fixture loading and the live llama-server benchmark loop (#38).
//!
//! Fixture loading is hermetic and unit-tested; the live benchmark requires a
//! running `llama-server` and is exercised only when the harness is pointed at
//! an endpoint.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use ravn_agent::inference::{build_user_prompt, parse_explanation, SYSTEM_PROMPT};
use ravn_core::Event;
use serde::Deserialize;

use crate::report::Row;
use crate::rubric::{self, Score};

/// The shared prompt-fixture directory (reused from the agent crate, #39).
pub fn default_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ravn-agent/tests/fixtures/prompts")
}

/// Load `(name, Event)` pairs from every `*.event.json` in `dir`, sorted by name.
pub fn load_events(dir: &Path) -> Result<Vec<(String, Event)>> {
    let mut out = Vec::new();
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading fixtures dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(".event.json") else {
            continue;
        };
        let json = std::fs::read_to_string(entry.path())
            .with_context(|| format!("reading {file_name}"))?;
        let event: Event =
            serde_json::from_str(&json).with_context(|| format!("parsing {file_name} as Event"))?;
        out.push((name.to_string(), event));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// --- llama.cpp response shapes -------------------------------------------------

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    timings: Option<Timings>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

/// llama.cpp per-request timings (present on recent server builds).
#[derive(Deserialize, Default)]
struct Timings {
    #[serde(default)]
    prompt_per_second: Option<f64>,
    #[serde(default)]
    predicted_per_second: Option<f64>,
}

/// Per-fixture benchmark outcome.
pub struct EventResult {
    pub name: String,
    pub latency_ms: f64,
    pub prompt_tps: f64,
    pub gen_tps: f64,
    pub score: Score,
}

/// Benchmark a single event against a model, measuring latency, throughput and
/// the rubric quality of the parsed explanation.
pub async fn bench_event(
    http: &reqwest::Client,
    endpoint: &str,
    model: &str,
    name: &str,
    event: &Event,
) -> Result<EventResult> {
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": build_user_prompt(event) },
        ],
        "temperature": 0.2,
        "max_tokens": 320,
        "response_format": { "type": "json_object" },
    });

    let started = Instant::now();
    let resp = http
        .post(format!("{endpoint}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .context("inference request failed")?
        .error_for_status()
        .context("inference returned an error status")?
        .json::<ChatResponse>()
        .await
        .context("decoding inference response")?;
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

    let content = resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    let (text, check) = parse_explanation(&content);
    let score = rubric::score(event, &text, check.as_deref());

    let timings = resp.timings.unwrap_or_default();
    Ok(EventResult {
        name: name.to_string(),
        latency_ms,
        prompt_tps: timings.prompt_per_second.unwrap_or(0.0),
        gen_tps: timings.predicted_per_second.unwrap_or(0.0),
        score,
    })
}

/// Benchmark a model across all fixtures, returning an aggregated [`Row`].
pub async fn bench_model(
    http: &reqwest::Client,
    endpoint: &str,
    model: &str,
    fixtures: &[(String, Event)],
) -> Result<Row> {
    let mut results = Vec::with_capacity(fixtures.len());
    for (name, event) in fixtures {
        let r = bench_event(http, endpoint, model, name, event).await?;
        println!(
            "  {:<28} quality={:.2}  {:.0}ms  gen={:.1} tok/s",
            r.name,
            r.score.overall(),
            r.latency_ms,
            r.gen_tps
        );
        results.push(r);
    }

    let n = results.len().max(1) as f64;
    let mean = |f: &dyn Fn(&EventResult) -> f64| results.iter().map(f).sum::<f64>() / n;
    Ok(Row {
        model: if model.is_empty() { "<server default>".into() } else { model.into() },
        prompt_tps: mean(&|r| r.prompt_tps),
        gen_tps: mean(&|r| r.gen_tps),
        latency_ms: mean(&|r| r.latency_ms),
        mem_mb: sample_memory_mb(http, endpoint).await,
        quality: mean(&|r| r.score.overall()),
        fixtures: results.len(),
    })
}

/// Best-effort RSS of the llama-server, scraped from its Prometheus `/metrics`
/// (`--metrics`). Returns `None` if metrics are unavailable.
pub async fn sample_memory_mb(http: &reqwest::Client, endpoint: &str) -> Option<f64> {
    let body = http.get(format!("{endpoint}/metrics")).send().await.ok()?.text().await.ok()?;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("process_resident_memory_bytes ") {
            if let Ok(bytes) = rest.trim().parse::<f64>() {
                return Some(bytes / (1024.0 * 1024.0));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_all_prompt_fixtures() {
        let events = load_events(&default_fixtures_dir()).expect("load fixtures");
        // Mirrors the #39 corpus: one event per detection source.
        assert!(events.len() >= 5, "expected >=5 fixtures, got {}", events.len());
        assert!(events.iter().any(|(n, _)| n == "failed_unit_nginx"));
        // Every fixture renders a non-trivial prompt and yields salient tokens.
        for (name, event) in &events {
            assert!(!build_user_prompt(event).is_empty(), "{name} empty prompt");
            assert!(!rubric::salient_tokens(event).is_empty(), "{name} no tokens");
        }
    }
}
