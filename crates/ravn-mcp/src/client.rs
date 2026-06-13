//! A thin HTTP client for the Ravn control plane's read API (and the two gated
//! mutating endpoints).
//!
//! The control-plane API *is* the schema of record (`portal/openapi.json`), so
//! for the list/topology/event reads we pass JSON through verbatim — the MCP
//! tool's job is to relay the control plane's answer, not to re-model it, which
//! keeps this bridge resilient as the API grows fields. The one place we parse
//! into typed `ravn-core` values is the remediation list, where the typed
//! [`ravn_core::RemediationRecord`] lets us cleanly split *pending proposals*
//! from already-decided ones for the read-only `list_remediation_proposals`
//! tool.

use anyhow::{anyhow, Context, Result};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};
use serde_json::Value;

/// A client bound to one control-plane base URL and an optional bearer token.
///
/// The token, when present, is sent on every request. Read endpoints accept a
/// viewer (or admin) token; the gated mutating endpoints require admin. Auth may
/// be disabled control-plane-side, in which case no token is needed.
#[derive(Clone)]
pub struct ControlPlaneClient {
    http: Client,
    base_url: String,
    token: Option<String>,
}

impl ControlPlaneClient {
    /// Build a client for `base_url` (e.g. `http://127.0.0.1:8080`), optionally
    /// presenting `token` as a bearer credential.
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("ravn-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        let base_url = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self {
            http,
            base_url,
            token,
        })
    }

    /// Issue a GET and return the parsed JSON body, mapping non-2xx into an
    /// error that carries the status and (truncated) body for diagnosis.
    async fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.http.get(&url);
        if let Some(token) = &self.token {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let resp = req.send().await.with_context(|| format!("GET {url}"))?;
        Self::json_or_error(resp, &url).await
    }

    /// Issue a POST with no body to a mutating endpoint and return the parsed
    /// JSON body.
    async fn post(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self
            .http
            .post(&url)
            .header(CONTENT_TYPE, "application/json");
        if let Some(token) = &self.token {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let resp = req.send().await.with_context(|| format!("POST {url}"))?;
        Self::json_or_error(resp, &url).await
    }

    /// Turn a response into JSON, or a descriptive error on a non-success status.
    /// 401/403 are called out explicitly because they are the common operator
    /// mistake (missing/insufficient token).
    async fn json_or_error(resp: reqwest::Response, url: &str) -> Result<Value> {
        let status = resp.status();
        if status.is_success() {
            // 204 No Content (and other empty bodies) decode to JSON null.
            let body = resp
                .text()
                .await
                .with_context(|| format!("reading body of {url}"))?;
            if body.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&body)
                .with_context(|| format!("parsing JSON body of {url}"));
        }
        let body = resp.text().await.unwrap_or_default();
        let hint = match status {
            StatusCode::UNAUTHORIZED => {
                " (missing or invalid token — set RAVN_MCP_TOKEN to a control-plane bearer token)"
            }
            StatusCode::FORBIDDEN => {
                " (token lacks the required role — mutations need an admin-scoped token)"
            }
            _ => "",
        };
        Err(anyhow!(
            "control plane returned {status} for {url}{hint}: {}",
            truncate(&body, 500)
        ))
    }

    // ---- Read-only endpoints -------------------------------------------------

    /// `GET /api/agents` — all registered agents with status and labels.
    pub async fn list_agents(&self) -> Result<Value> {
        self.get("/api/agents").await
    }

    /// `GET /api/events?limit=N` — recent events, newest first.
    pub async fn list_events(&self, limit: u32) -> Result<Value> {
        self.get(&format!("/api/events?limit={limit}")).await
    }

    /// `GET /api/topology[?group_by=KEY]` — the fleet shaped for the diagram.
    pub async fn topology(&self, group_by: Option<&str>) -> Result<Value> {
        let path = match group_by {
            Some(key) => format!("/api/topology?group_by={}", urlencode(key)),
            None => "/api/topology".to_string(),
        };
        self.get(&path).await
    }

    /// `GET /api/remediations` — the full remediation audit trail. The caller
    /// filters this for *pending proposals* in the read-only tool.
    pub async fn list_remediations(&self) -> Result<Value> {
        self.get("/api/remediations").await
    }

    // ---- Gated mutating endpoints -------------------------------------------

    /// `POST /api/remediations/{id}/approve` — sign + enqueue the command for a
    /// pending proposal. Admin-only on the control plane; only reachable when the
    /// MCP server was started with `--allow-mutations`.
    pub async fn approve_remediation(&self, id: &str) -> Result<Value> {
        self.post(&format!("/api/remediations/{}/approve", urlencode(id)))
            .await
    }

    /// `POST /api/remediations/{id}/reject` — reject a pending proposal. Admin
    /// only on the control plane; gated behind `--allow-mutations`.
    pub async fn reject_remediation(&self, id: &str) -> Result<Value> {
        self.post(&format!("/api/remediations/{}/reject", urlencode(id)))
            .await
    }
}

/// Percent-encode the characters that matter for a single path/query segment.
/// The values we pass (UUIDs, label keys) are simple, so a minimal encoder is
/// enough and avoids pulling a dependency.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Truncate a string to `max` bytes for error messages, appending an ellipsis.
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

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let c = ControlPlaneClient::new("http://x:8080/", None).unwrap();
        assert_eq!(c.base_url, "http://x:8080");
    }

    #[test]
    fn urlencode_passes_uuid_through() {
        let id = "018f0000-0000-7000-8000-000000000000";
        assert_eq!(urlencode(id), id);
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(urlencode("a/b?c"), "a%2Fb%3Fc");
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 10), "abc");
        assert!(truncate("abcdefghij", 3).starts_with("abc"));
    }
}
