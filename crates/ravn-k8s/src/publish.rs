//! Outbound transport selection: NATS (default) or authenticated HTTP ingest.
//!
//! The agent publishes to NATS unless `RAVN_INGEST_URL` is set, in which case
//! it uses the authenticated HTTP ingest endpoint with its ServiceAccount
//! token (#57). Both controller and node-agent go through this one type so the
//! choice lives in a single place.

use ravn_core::Message;

use crate::config::Config;
use crate::http::HttpPublisher;
use crate::nats::NatsPublisher;

/// A connected outbound publisher.
pub enum Publisher {
    Nats(NatsPublisher),
    Http(HttpPublisher),
}

impl Publisher {
    /// Connect the transport implied by the config: HTTP ingest when
    /// `ingest_url` is set, otherwise NATS.
    pub async fn connect(config: &Config) -> anyhow::Result<Self> {
        match &config.ingest_url {
            Some(url) => {
                tracing::info!(url, "publishing via authenticated HTTP ingest");
                Ok(Self::Http(HttpPublisher::new(url.clone(), config.sa_token_file.clone())?))
            }
            None => {
                tracing::info!(url = %config.nats_url, "publishing via NATS");
                Ok(Self::Nats(NatsPublisher::connect(&config.nats_url, config.agent_id).await?))
            }
        }
    }

    /// Publish a message over the selected transport.
    pub async fn publish(&self, message: &Message) -> anyhow::Result<()> {
        match self {
            Publisher::Nats(p) => p.publish(message).await,
            Publisher::Http(p) => p.publish(message).await,
        }
    }
}
