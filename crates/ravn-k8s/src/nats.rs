//! Minimal NATS publisher: the controller emits [`Message`]s on the same
//! `ravn.messages.<id>` subject the host agent uses, so the control plane's
//! existing ingestion (`ravn.messages.*`) picks them up unchanged (#22).

use anyhow::Context;
use ravn_core::{AgentId, Message};

/// A connected outbound NATS publisher bound to the agent's subject.
pub struct NatsPublisher {
    client: async_nats::Client,
    subject: String,
}

impl NatsPublisher {
    /// Connect to NATS, retrying the initial connect with backoff.
    pub async fn connect(url: &str, agent_id: AgentId) -> anyhow::Result<Self> {
        let client = async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .connect(url)
            .await
            .with_context(|| format!("connecting to NATS at {url}"))?;
        Ok(Self { client, subject: format!("ravn.messages.{}", agent_id.0) })
    }

    /// Publish a message and flush, so it leaves the process before we move on.
    pub async fn publish(&self, message: &Message) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(message).context("serializing message")?;
        self.client
            .publish(self.subject.clone(), payload.into())
            .await
            .context("publishing to NATS")?;
        self.client.flush().await.context("flushing NATS")?;
        Ok(())
    }
}
