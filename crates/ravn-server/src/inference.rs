//! Control-plane inference (#58): request a plain-language explanation for a
//! Kubernetes event from a shared, configurable, OpenAI-compatible inference
//! endpoint.
//!
//! This is the LLM "last mile" for K8s signals. The K8s controller and
//! node-agent are detection-only (no per-pod/per-node model), so their events
//! arrive without an explanation; the control plane fills it in **off the
//! detection path** (the alert has already fired) by calling a shared service.
//! If inference is slow or unreachable the event simply keeps its deterministic
//! title — safe failure.

use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use ravn_core::{Event, Explanation, Payload};
use serde::Deserialize;

/// System prompt: constrains the model to a short explanation + one check,
/// returned as JSON so we can split the two reliably.
const SYSTEM_PROMPT: &str = "You are Ravn, a Kubernetes operations assistant. Given a \
single already-detected cluster event, explain in 2-3 plain sentences what it most \
likely means and why it matters. Then suggest one concrete kubectl command or check an \
operator could run. You never decide whether something is wrong — the event is already \
flagged; you only explain it. Reply ONLY with a JSON object: \
{\"explanation\": \"...\", \"suggested_check\": \"...\"}.";

/// A configured client for an OpenAI-compatible chat-completions endpoint.
pub struct InferenceClient {
    endpoint: String,
    model: String,
    api_key: Option<String>,
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
    /// Build a client. `endpoint` is the OpenAI-compatible base URL (e.g.
    /// `http://inference.svc/v1`); `/chat/completions` is appended per request.
    pub fn new(endpoint: String, model: String, api_key: Option<String>, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("building HTTP client");
        Self { endpoint: endpoint.trim_end_matches('/').to_string(), model, api_key, http }
    }

    /// Request an explanation for an event. Returns `Err` on any transport or
    /// parse failure so the caller can log and move on (the alert already fired).
    pub async fn explain(&self, event: &Event) -> anyhow::Result<Explanation> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": build_user_prompt(event) },
            ],
            "max_tokens": 512,
            "temperature": 0.2,
        });

        let mut req = self.http.post(format!("{}/chat/completions", self.endpoint)).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.context("calling inference endpoint")?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("inference endpoint returned {status}");
        }
        let parsed: ChatResponse = resp.json().await.context("parsing inference response")?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .context("inference response had no choices")?;

        let (text, suggested_check) = parse_explanation(&content);
        Ok(Explanation { text, suggested_check, model: self.model.clone(), generated_at: Utc::now() })
    }
}

/// Build the user prompt describing a K8s event for the model.
fn build_user_prompt(event: &Event) -> String {
    let head = format!("Event: {} (severity: {:?}).", event.title, event.severity);
    let detail = match &event.payload {
        Payload::KubeWorkload(p) => {
            let container = p.container.as_deref().map(|c| format!(", container {c}")).unwrap_or_default();
            let msg = p.message.as_deref().unwrap_or("");
            format!(
                "Kubernetes workload signal: {} {}/{}{} — reason {}. {}",
                if p.object_kind.is_empty() { "object" } else { &p.object_kind },
                p.namespace,
                p.name,
                container,
                p.reason,
                msg
            )
        }
        Payload::KubeNode(p) => format!(
            "Kubernetes node signal: node {} condition {}. {}",
            p.node,
            p.condition,
            p.message.as_deref().unwrap_or("")
        ),
        // Non-K8s events are explained on the host agent; we only reach here for
        // K8s sources, but degrade gracefully.
        _ => event.title.clone(),
    };
    format!("{head}\n{detail}")
}

/// Parse the model's reply into (explanation, suggested_check), tolerating a
/// bare JSON object, JSON embedded in prose, or plain text.
pub fn parse_explanation(content: &str) -> (String, Option<String>) {
    let from_json = |v: &serde_json::Value| -> Option<(String, Option<String>)> {
        let text = v.get("explanation")?.as_str()?.trim().to_string();
        let check = v
            .get("suggested_check")
            .and_then(|c| c.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Some((text, check))
    };

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(parsed) = from_json(&v) {
            return parsed;
        }
    }
    if let (Some(start), Some(end)) = (content.find('{'), content.rfind('}')) {
        if start < end {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content[start..=end]) {
                if let Some(parsed) = from_json(&v) {
                    return parsed;
                }
            }
        }
    }
    // Lenient last resort: a verbose small model can hit max_tokens and emit
    // *truncated* JSON that won't parse. Pull the string fields out by hand so
    // the portal shows the sentence, not raw `{"explanation": ...`.
    if let Some(text) = extract_string_field(content, "explanation") {
        let check = extract_string_field(content, "suggested_check").filter(|s| !s.is_empty());
        return (text.trim().to_string(), check);
    }
    (content.trim().to_string(), None)
}

/// Extract a JSON string field's value by name, tolerating a truncated object
/// (returns what was read so far if the closing quote is missing). Handles the
/// common backslash escapes.
fn extract_string_field(content: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let rest = &content[content.find(&needle)? + needle.len()..];
    let after_colon = rest[rest.find(':')? + 1..].trim_start();
    let mut chars = after_colon.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            out.push(match c {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other => other, // covers " \ / and anything else
            });
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    // Truncated before the closing quote — return the partial value.
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{AgentId, KubeNodePayload, KubeWorkloadPayload, Severity};
    use uuid::Uuid;

    fn event(payload: Payload, title: &str) -> Event {
        let now = Utc::now();
        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::nil()),
            host: "ravn-dev".into(),
            severity: Severity::Error,
            title: title.into(),
            category_hints: vec![],
            payload,
        }
    }

    #[test]
    fn parses_bare_json() {
        let (text, check) = parse_explanation(r#"{"explanation":"It crashed.","suggested_check":"kubectl logs"}"#);
        assert_eq!(text, "It crashed.");
        assert_eq!(check.as_deref(), Some("kubectl logs"));
    }

    #[test]
    fn parses_json_embedded_in_prose() {
        let (text, check) =
            parse_explanation("Sure!\n{\"explanation\": \"OOM.\", \"suggested_check\": \"free -m\"} hope that helps");
        assert_eq!(text, "OOM.");
        assert_eq!(check.as_deref(), Some("free -m"));
    }

    #[test]
    fn recovers_explanation_from_truncated_json() {
        // A verbose model hit max_tokens mid-object: the explanation closed but
        // suggested_check was cut off. We must still show the sentence.
        let content =
            r#"{"explanation": "The pod is crash-looping because the container keeps exiting.", "s"#;
        let (text, check) = parse_explanation(content);
        assert_eq!(text, "The pod is crash-looping because the container keeps exiting.");
        assert!(check.is_none());
        assert!(!text.starts_with('{'), "must not leak raw JSON into the text");
    }

    #[test]
    fn recovers_both_fields_from_unterminated_object() {
        // Whole object never closed (no final }), both string values intact.
        let content = r#"{"explanation": "OOMKilled.", "suggested_check": "kubectl top pod"#;
        let (text, check) = parse_explanation(content);
        assert_eq!(text, "OOMKilled.");
        assert_eq!(check.as_deref(), Some("kubectl top pod"));
    }

    #[test]
    fn falls_back_to_plain_text() {
        let (text, check) = parse_explanation("just some prose, no json");
        assert_eq!(text, "just some prose, no json");
        assert!(check.is_none());
    }

    #[test]
    fn empty_suggested_check_becomes_none() {
        let (_t, check) = parse_explanation(r#"{"explanation":"x","suggested_check":""}"#);
        assert!(check.is_none());
    }

    #[test]
    fn workload_prompt_mentions_namespace_reason_container() {
        let p = Payload::KubeWorkload(KubeWorkloadPayload {
            namespace: "ravn-test".into(),
            object_kind: "Pod".into(),
            name: "crasher".into(),
            reason: "CrashLoopBackOff".into(),
            message: Some("back-off restarting".into()),
            container: Some("boom".into()),
            ..Default::default()
        });
        let prompt = build_user_prompt(&event(p, "CrashLoopBackOff: Pod ravn-test/crasher"));
        assert!(prompt.contains("ravn-test"));
        assert!(prompt.contains("CrashLoopBackOff"));
        assert!(prompt.contains("boom"));
    }

    #[test]
    fn node_prompt_mentions_node_and_condition() {
        let p = Payload::KubeNode(KubeNodePayload {
            node: "k3d-agent-0".into(),
            condition: "MemoryPressure".into(),
            message: Some("kubelet under pressure".into()),
            ..Default::default()
        });
        let prompt = build_user_prompt(&event(p, "MemoryPressure on node k3d-agent-0"));
        assert!(prompt.contains("k3d-agent-0"));
        assert!(prompt.contains("MemoryPressure"));
    }
}
