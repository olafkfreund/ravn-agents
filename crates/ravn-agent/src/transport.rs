//! Outbound transport: publish [`Message`]s to the control plane.
//!
//! NATS is the primary transport (#22); a plain-WebSocket fallback lets M0 run
//! without a broker. The scheme of the server URL selects which is used:
//! `nats://` / `tls://` → NATS, `ws://` / `wss://` → WebSocket.

use std::time::Duration;

use anyhow::Context;
use futures_util::SinkExt;
use ravn_core::{Heartbeat, Message};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

/// A connected outbound transport.
pub enum Transport {
    Nats(NatsTransport),
    // Boxed: a WsTransport holds a whole WebSocket stream, far larger than the
    // NATS client handle.
    Ws(Box<WsTransport>),
}

impl Transport {
    /// Connect the transport implied by the URL scheme.
    pub async fn connect(server_url: &str, agent_id: Uuid) -> anyhow::Result<Self> {
        if server_url.starts_with("nats://") || server_url.starts_with("tls://") {
            Ok(Self::Nats(NatsTransport::connect(server_url, agent_id).await?))
        } else if server_url.starts_with("ws://") || server_url.starts_with("wss://") {
            Ok(Self::Ws(Box::new(WsTransport::connect(server_url).await?)))
        } else {
            anyhow::bail!("unsupported transport URL scheme: {server_url}");
        }
    }

    /// Publish a message, awaiting confirmation that it left the process.
    pub async fn publish(&self, message: &Message) -> anyhow::Result<()> {
        match self {
            Transport::Nats(t) => t.publish(message).await,
            Transport::Ws(t) => t.publish(message).await,
        }
    }

    /// Publish a heartbeat (separate subject from messages).
    pub async fn publish_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        match self {
            Transport::Nats(t) => t.publish_heartbeat(hb).await,
            Transport::Ws(t) => t.publish_heartbeat(hb).await,
        }
    }
}

/// NATS transport. async-nats reconnects automatically with backoff.
pub struct NatsTransport {
    client: async_nats::Client,
    subject: String,
    heartbeat_subject: String,
}

impl NatsTransport {
    pub async fn connect(url: &str, agent_id: Uuid) -> anyhow::Result<Self> {
        let client = async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .connect(url)
            .await
            .with_context(|| format!("connecting to NATS at {url}"))?;

        Ok(Self {
            client,
            subject: format!("ravn.messages.{agent_id}"),
            heartbeat_subject: format!("ravn.heartbeat.{agent_id}"),
        })
    }

    pub async fn publish(&self, message: &Message) -> anyhow::Result<()> {
        self.publish_to(&self.subject, message).await
    }

    pub async fn publish_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        self.publish_to(&self.heartbeat_subject, hb).await
    }

    async fn publish_to<T: serde::Serialize>(&self, subject: &str, value: &T) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(value).context("serializing payload")?;
        self.client
            .publish(subject.to_string(), payload.into())
            .await
            .context("publishing to NATS")?;
        // Our emissions are bursty and the process may be short-lived, so flush
        // to make sure the message actually leaves before we move on.
        self.client.flush().await.context("flushing NATS")?;
        Ok(())
    }
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Plain-WebSocket fallback transport for running without a broker.
pub struct WsTransport {
    url: String,
    stream: Mutex<WsStream>,
}

impl WsTransport {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let stream = connect_with_backoff(url).await?;
        Ok(Self { url: url.to_string(), stream: Mutex::new(stream) })
    }

    pub async fn publish(&self, message: &Message) -> anyhow::Result<()> {
        self.send_json(message).await
    }

    pub async fn publish_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        self.send_json(hb).await
    }

    async fn send_json<T: serde::Serialize>(&self, value: &T) -> anyhow::Result<()> {
        let text = serde_json::to_string(value).context("serializing payload")?;
        let mut stream = self.stream.lock().await;

        if let Err(error) = stream.send(WsMessage::text(text.clone())).await {
            // The connection likely dropped; reconnect once and resend.
            tracing::warn!(%error, "WebSocket send failed; reconnecting");
            *stream = connect_with_backoff(&self.url).await?;
            stream
                .send(WsMessage::text(text))
                .await
                .context("WebSocket resend after reconnect")?;
        }

        Ok(())
    }
}

/// Connect to a WebSocket URL, retrying with exponential backoff.
async fn connect_with_backoff(url: &str) -> anyhow::Result<WsStream> {
    let mut delay = Duration::from_millis(200);
    let max_attempts = 5;

    for attempt in 1..=max_attempts {
        match connect_async(url).await {
            Ok((stream, _response)) => return Ok(stream),
            Err(error) if attempt == max_attempts => {
                return Err(anyhow::Error::new(error)
                    .context(format!("connecting to {url} after {attempt} attempts")));
            }
            Err(error) => {
                tracing::warn!(%error, attempt, "WebSocket connect failed; retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
        }
    }

    unreachable!("loop returns on the final attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use futures_util::StreamExt;
    use ravn_core::{AgentId, Event, JournaldPayload, Payload, Severity};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn sample_message() -> Message {
        let now = Utc::now();
        Message::new(Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::now_v7()),
            host: "test".into(),
            severity: Severity::Info,
            title: "hello".into(),
            category_hints: vec![],
            payload: Payload::Journald(JournaldPayload {
                message: "ping".into(),
                ..Default::default()
            }),
        })
    }

    #[tokio::test]
    async fn ws_transport_delivers_message() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // In-process WebSocket server that captures the first frame.
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(sock).await.unwrap();
            if let Some(Ok(msg)) = ws.next().await {
                let _ = tx.send(msg.into_text().unwrap().as_str().to_string());
            }
        });

        let url = format!("ws://{addr}");
        let transport = Transport::connect(&url, Uuid::now_v7()).await.unwrap();
        let message = sample_message();
        transport.publish(&message).await.unwrap();

        let received = rx.await.unwrap();
        let decoded: Message = serde_json::from_str(&received).unwrap();
        assert_eq!(decoded, message);
    }

    #[tokio::test]
    async fn unknown_scheme_is_rejected() {
        let result = Transport::connect("http://example.com", Uuid::now_v7()).await;
        let err = match result {
            Ok(_) => panic!("expected an error for an unsupported scheme"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unsupported transport URL scheme"));
    }
}
