//! Reactive inference (#16).
//!
//! Turns a flagged [`Event`] into a plain-language [`Explanation`] plus a
//! suggested check by calling a local `llama-server` over its OpenAI-compatible
//! API. The prompt is tight and asks for JSON output. This is best-effort and
//! off the detection decision: if the model is slow or unreachable the event is
//! still emitted, just without an explanation.

use std::time::Duration;

use chrono::Utc;
use ravn_core::{Event, Explanation, Payload};
use serde::Deserialize;

const SYSTEM_PROMPT: &str = "You are Ravn, a Linux operations assistant. Given a \
flagged system event, explain in 2-3 plain sentences what most likely happened \
and why it matters, then suggest ONE concrete command or check an operator can \
run. Be factual and concise; do not invent details not supported by the event. \
Respond ONLY as a JSON object: {\"explanation\": string, \"suggested_check\": string}.";

/// Calls a local llama-server to explain events.
pub struct InferenceClient {
    endpoint: String,
    model: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

impl InferenceClient {
    pub fn new(endpoint: String, model: String, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        Self { endpoint: endpoint.trim_end_matches('/').to_string(), model, http }
    }

    /// Best-effort explanation for an event. Returns `None` on any failure.
    pub async fn explain(&self, event: &Event) -> Option<Explanation> {
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

        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .json(&body)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "inference request failed");
            return None;
        }

        let content = resp
            .json::<ChatResponse>()
            .await
            .ok()?
            .choices
            .into_iter()
            .next()?
            .message
            .content;

        let (text, suggested_check) = parse_explanation(&content);
        if text.trim().is_empty() {
            return None;
        }
        Some(Explanation { text, suggested_check, model: self.model.clone(), generated_at: Utc::now() })
    }
}

/// Build the user prompt from an event, including a bounded slice of context.
fn build_user_prompt(event: &Event) -> String {
    let mut s = format!(
        "Severity: {:?}\nSource: {:?}\nHost: {}\nTitle: {}\n",
        event.severity,
        event.source(),
        event.host,
        event.title,
    );
    match &event.payload {
        Payload::Journald(p) => {
            if let Some(u) = &p.unit {
                s.push_str(&format!("Unit: {u}\n"));
            }
            s.push_str(&format!("Message: {}\n", p.message));
        }
        Payload::FailedUnit(p) => {
            s.push_str(&format!("Failed unit: {} (result: {})\n", p.unit, p.result));
            if !p.recent_log.is_empty() {
                s.push_str("Recent log:\n");
                for line in p.recent_log.iter().take(10) {
                    s.push_str(&format!("  {line}\n"));
                }
            }
        }
        Payload::ConfigDrift(p) => {
            s.push_str(&format!("Changed file: {}\n", p.path));
            if let Some(diff) = &p.diff {
                s.push_str("Diff:\n");
                s.push_str(&truncate(diff, 1500));
                s.push('\n');
            }
        }
        Payload::Auth(p) => {
            s.push_str(&format!("Action: {} (succeeded: {})\n", p.action, p.succeeded));
            if let Some(u) = &p.user {
                s.push_str(&format!("User: {u}\n"));
            }
            if let Some(r) = &p.remote_addr {
                s.push_str(&format!("Remote: {r}\n"));
            }
        }
        Payload::Update(p) => {
            s.push_str(&format!("Update via {}: {:?} -> {:?}\n", p.mechanism, p.from, p.to));
            if !p.changes.is_empty() {
                s.push_str("Changes:\n");
                for line in p.changes.iter().take(20) {
                    s.push_str(&format!("  {line}\n"));
                }
            }
        }
    }
    s
}

/// Parse the model's content into (explanation, suggested_check), tolerating
/// JSON wrapped in prose or missing fields.
fn parse_explanation(content: &str) -> (String, Option<String>) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(parsed) = extract(&v) {
            return parsed;
        }
    }
    // Find the first {...} block and try that.
    if let (Some(start), Some(end)) = (content.find('{'), content.rfind('}')) {
        if end > start {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content[start..=end]) {
                if let Some(parsed) = extract(&v) {
                    return parsed;
                }
            }
        }
    }
    // Fall back to treating the whole thing as the explanation.
    (content.trim().to_string(), None)
}

fn extract(v: &serde_json::Value) -> Option<(String, Option<String>)> {
    let text = v.get("explanation")?.as_str()?.trim().to_string();
    let check = v
        .get("suggested_check")
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((text, check))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ravn_core::{AgentId, FailedUnitPayload, Severity};
    use uuid::Uuid;

    fn event() -> Event {
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
                recent_log: vec!["bind() to 0.0.0.0:80 failed".into()],
                ..Default::default()
            }),
        }
    }

    #[test]
    fn prompt_includes_event_context() {
        let p = build_user_prompt(&event());
        assert!(p.contains("web-01"));
        assert!(p.contains("nginx.service"));
        assert!(p.contains("exit-code"));
        assert!(p.contains("bind() to 0.0.0.0:80 failed"));
    }

    #[test]
    fn parses_clean_json() {
        let (text, check) = parse_explanation(r#"{"explanation":"nginx died","suggested_check":"systemctl status nginx"}"#);
        assert_eq!(text, "nginx died");
        assert_eq!(check.as_deref(), Some("systemctl status nginx"));
    }

    #[test]
    fn parses_json_wrapped_in_prose() {
        let (text, check) = parse_explanation("Sure!\n{\"explanation\": \"port in use\", \"suggested_check\": \"ss -ltnp\"}\nHope that helps.");
        assert_eq!(text, "port in use");
        assert_eq!(check.as_deref(), Some("ss -ltnp"));
    }

    #[test]
    fn falls_back_to_plain_text() {
        let (text, check) = parse_explanation("just a sentence, no json");
        assert_eq!(text, "just a sentence, no json");
        assert!(check.is_none());
    }

    // ---- Golden prompt-regression fixtures (#39) ----------------------------
    //
    // These scan `tests/fixtures/` so contributors can add cases by dropping in
    // data files (see that directory's README). `RAVN_BLESS=1` regenerates the
    // `*.prompt.txt` goldens from the current `build_user_prompt`.

    use std::path::{Path, PathBuf};

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Sorted base names of files in `dir` whose name ends with `suffix`.
    fn case_names(dir: &Path, suffix: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read fixtures dir {}: {e}", dir.display()))
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(suffix).map(str::to_string))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn fixture_prompts_render_to_golden() {
        let dir = fixtures_dir().join("prompts");
        let bless = std::env::var_os("RAVN_BLESS").is_some();
        let names = case_names(&dir, ".event.json");
        assert!(!names.is_empty(), "no prompt fixtures found in {}", dir.display());

        for name in names {
            let event_json = std::fs::read_to_string(dir.join(format!("{name}.event.json")))
                .unwrap_or_else(|e| panic!("read {name}.event.json: {e}"));
            let event: Event = serde_json::from_str(&event_json)
                .unwrap_or_else(|e| panic!("parse {name}.event.json as Event: {e}"));
            let rendered = build_user_prompt(&event);

            let golden_path = dir.join(format!("{name}.prompt.txt"));
            if bless {
                std::fs::write(&golden_path, &rendered)
                    .unwrap_or_else(|e| panic!("bless {name}.prompt.txt: {e}"));
                continue;
            }
            let expected = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
                panic!("missing golden {name}.prompt.txt ({e}); run with RAVN_BLESS=1")
            });
            assert_eq!(
                rendered, expected,
                "prompt for fixture '{name}' drifted from golden; \
                 if intended, re-bless with RAVN_BLESS=1"
            );
        }
    }

    #[test]
    fn fixture_explanations_parse_as_expected() {
        let dir = fixtures_dir().join("explanations");
        let names = case_names(&dir, ".response.txt");
        assert!(!names.is_empty(), "no explanation fixtures found in {}", dir.display());

        for name in names {
            let response = std::fs::read_to_string(dir.join(format!("{name}.response.txt")))
                .unwrap_or_else(|e| panic!("read {name}.response.txt: {e}"));
            let expected_json = std::fs::read_to_string(dir.join(format!("{name}.expected.json")))
                .unwrap_or_else(|e| panic!("read {name}.expected.json: {e}"));
            let expected: serde_json::Value = serde_json::from_str(&expected_json)
                .unwrap_or_else(|e| panic!("parse {name}.expected.json: {e}"));

            let (text, check) = parse_explanation(&response);
            assert_eq!(
                text,
                expected["explanation"].as_str().unwrap_or_default(),
                "explanation for fixture '{name}' did not match"
            );
            let expected_check = expected["suggested_check"].as_str();
            assert_eq!(
                check.as_deref(),
                expected_check,
                "suggested_check for fixture '{name}' did not match"
            );
        }
    }
}
