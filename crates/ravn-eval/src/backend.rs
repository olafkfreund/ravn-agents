//! Pluggable model backends (#157).
//!
//! The harness scores *explanations*, not a particular inference stack, so the
//! model is behind a trait. Two backends ship:
//!
//! - [`LlamaServerBackend`] — talks to a live `llama-server` over its
//!   OpenAI-compatible API, measuring real latency and (best-effort) RAM. This
//!   is the path you run on a CPU box to produce real numbers.
//! - [`RecordedBackend`] — replays a recorded model run from
//!   `fixtures/recordings/<model>/`, so the harness produces a full scored
//!   table with zero model dependencies. This is what makes the run
//!   reproducible in CI / `nix flake check`, and the report clearly marks these
//!   rows as recorded.
//!
//! Both return a [`Completion`]: the raw model text plus the resource numbers
//! the runner aggregates.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use ravn_agent::inference::{build_user_prompt, SYSTEM_PROMPT};
use ravn_core::Event;
use serde::Deserialize;

/// One model response plus the resources it took to produce.
#[derive(Debug, Clone)]
pub struct Completion {
    /// Raw model content (still to be parsed by `parse_explanation`).
    pub content: String,
    /// End-to-end wall-clock latency for this event, in milliseconds.
    pub latency_ms: f64,
    /// Generation throughput (tokens/sec), if the backend reports it.
    pub gen_tps: Option<f64>,
    /// Prompt-processing throughput (tokens/sec), if reported.
    pub prompt_tps: Option<f64>,
}

/// A source of explanations for the harness to score. Object-safe so the runner
/// can hold `&dyn ModelBackend`.
#[async_trait]
pub trait ModelBackend: Send + Sync {
    /// Stable model label for the report (e.g. `qwen3-1.7b`).
    fn name(&self) -> &str;

    /// Whether results came from a recorded run rather than live inference.
    /// The report flags these so readers don't mistake them for measured.
    fn is_recorded(&self) -> bool;

    /// Provenance of a recorded run (CPU, quant, server build); empty for live
    /// backends. Surfaced in the site page so the numbers are reproducible.
    fn captured_on(&self) -> &str {
        ""
    }

    /// Resident memory of the model while serving, in MiB, if known.
    async fn memory_mb(&self) -> Option<f64>;

    /// Produce a completion for a single event.
    async fn complete(&self, event: &Event) -> Result<Completion>;
}

// --- Live llama-server backend -------------------------------------------------

/// Talks to a running `llama-server` (OpenAI-compatible API).
pub struct LlamaServerBackend {
    name: String,
    model: String,
    endpoint: String,
    http: reqwest::Client,
}

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

impl LlamaServerBackend {
    /// Build a backend for `model` served at `endpoint`. An empty `model` uses
    /// whatever the server has loaded; the row is then labelled by `name`.
    pub fn new(name: String, model: String, endpoint: String, timeout: Duration) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            name,
            model,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            http,
        })
    }
}

#[async_trait]
impl ModelBackend for LlamaServerBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_recorded(&self) -> bool {
        false
    }

    async fn memory_mb(&self) -> Option<f64> {
        // Best-effort RSS scraped from the server's Prometheus `/metrics`
        // (`--metrics`). Returns None if metrics are unavailable.
        let body = self
            .http
            .get(format!("{}/metrics", self.endpoint))
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("process_resident_memory_bytes ") {
                if let Ok(bytes) = rest.trim().parse::<f64>() {
                    return Some(bytes / (1024.0 * 1024.0));
                }
            }
        }
        None
    }

    async fn complete(&self, event: &Event) -> Result<Completion> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": build_user_prompt(event) },
            ],
            "temperature": 0.2,
            "max_tokens": 320,
            "response_format": { "type": "json_object" },
        });

        let started = Instant::now();
        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.endpoint))
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
        let timings = resp.timings.unwrap_or_default();
        Ok(Completion {
            content,
            latency_ms,
            gen_tps: timings.predicted_per_second,
            prompt_tps: timings.prompt_per_second,
        })
    }
}

// --- Recorded backend ----------------------------------------------------------

/// Metadata for a recorded model run, read from `<dir>/manifest.json`.
///
/// Captures the resource profile so the report can show realistic latency/RAM
/// for a run that was captured elsewhere (e.g. on a CPU laptop) and committed.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordingManifest {
    /// Model label for the report.
    pub model: String,
    /// Human note: where/how this run was captured (CPU, quant, server build).
    #[serde(default)]
    pub captured_on: String,
    /// Recorded mean per-event latency in ms (used for every event so the table
    /// reflects the captured profile without re-timing replay).
    pub latency_ms: f64,
    /// Recorded mean generation throughput (tokens/sec).
    #[serde(default)]
    pub gen_tps: Option<f64>,
    /// Recorded mean prompt-processing throughput (tokens/sec).
    #[serde(default)]
    pub prompt_tps: Option<f64>,
    /// Recorded resident memory while serving, in MiB.
    #[serde(default)]
    pub mem_mb: Option<f64>,
}

/// Replays a recorded run: `<dir>/<event_name>.txt` holds the raw model content
/// for each fixture, and `<dir>/manifest.json` carries the resource profile.
pub struct RecordedBackend {
    dir: PathBuf,
    manifest: RecordingManifest,
}

impl RecordedBackend {
    /// Load a recorded backend from a directory containing a `manifest.json`.
    pub fn load(dir: &Path) -> Result<Self> {
        let manifest_path = dir.join("manifest.json");
        let json = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?;
        let manifest: RecordingManifest = serde_json::from_str(&json)
            .with_context(|| format!("parsing {}", manifest_path.display()))?;
        Ok(Self { dir: dir.to_path_buf(), manifest })
    }

    /// The default recordings root shipped with the crate.
    pub fn recordings_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recordings")
    }
}

#[async_trait]
impl ModelBackend for RecordedBackend {
    fn name(&self) -> &str {
        &self.manifest.model
    }

    fn is_recorded(&self) -> bool {
        true
    }

    fn captured_on(&self) -> &str {
        &self.manifest.captured_on
    }

    async fn memory_mb(&self) -> Option<f64> {
        self.manifest.mem_mb
    }

    async fn complete(&self, event: &Event) -> Result<Completion> {
        // Recorded responses are keyed by the event's title-free fixture name;
        // the runner passes the event, so we key on a stable derived id: the
        // event UUID's last segment is unique per fixture. To keep the files
        // human-readable we instead key on the fixture name, supplied via the
        // event title fallback — see `complete_named`.
        //
        // The runner always uses `complete_named`; this trait method exists for
        // completeness and keys on the event id.
        let key = event.id.to_string();
        self.read_recording(&key)
    }
}

impl RecordedBackend {
    /// Load the recorded content for a fixture by its corpus name. Returns a
    /// [`Completion`] stamped with the manifest's resource profile.
    pub fn complete_named(&self, fixture_name: &str) -> Result<Completion> {
        self.read_recording(fixture_name)
    }

    fn read_recording(&self, key: &str) -> Result<Completion> {
        let path = self.dir.join(format!("{key}.txt"));
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading recording {}", path.display()))?;
        Ok(Completion {
            content,
            latency_ms: self.manifest.latency_ms,
            gen_tps: self.manifest.gen_tps,
            prompt_tps: self.manifest.prompt_tps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recordings_dir_has_at_least_three_models() {
        let root = RecordedBackend::recordings_dir();
        let count = std::fs::read_dir(&root)
            .unwrap_or_else(|e| panic!("read recordings dir {}: {e}", root.display()))
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("manifest.json").exists())
            .count();
        // #157 done-criterion: a scored comparison for at least three models.
        assert!(count >= 3, "expected >=3 recorded models, found {count}");
    }

    #[test]
    fn recorded_backend_loads_and_replays() {
        let dir = RecordedBackend::recordings_dir().join("qwen3-1.7b");
        let backend = RecordedBackend::load(&dir).expect("load recording");
        assert!(backend.is_recorded());
        let c = backend.complete_named("failed_unit_nginx").expect("replay");
        assert!(!c.content.trim().is_empty());
        assert!(c.latency_ms > 0.0);
    }
}
