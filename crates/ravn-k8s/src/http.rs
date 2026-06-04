//! Authenticated HTTP ingest transport (#57).
//!
//! Publishes [`Message`]s by POSTing them to the control plane's `/ingest`
//! endpoint with the pod's projected ServiceAccount token as a bearer
//! credential. The token is re-read from disk on each publish so kubelet token
//! rotation (projected tokens refresh well before expiry) is picked up without
//! a restart.

use anyhow::Context;
use ravn_core::Message;

/// HTTP publisher targeting the control plane's authenticated ingest endpoint.
pub struct HttpPublisher {
    client: reqwest::Client,
    ingest_url: String,
    token_file: String,
}

impl HttpPublisher {
    /// Build a publisher for the given ingest URL and SA-token file path.
    pub fn new(ingest_url: String, token_file: String) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .context("building HTTP client")?;
        Ok(Self { client, ingest_url, token_file })
    }

    /// POST a message with the current ServiceAccount token.
    pub async fn publish(&self, message: &Message) -> anyhow::Result<()> {
        let token = tokio::fs::read_to_string(&self.token_file)
            .await
            .with_context(|| format!("reading ServiceAccount token at {}", self.token_file))?;

        let response = self
            .client
            .post(&self.ingest_url)
            .bearer_auth(token.trim())
            .json(message)
            .send()
            .await
            .with_context(|| format!("POSTing to {}", self.ingest_url))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("ingest endpoint returned {status}");
        }
        Ok(())
    }
}
