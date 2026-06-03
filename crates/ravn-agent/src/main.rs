//! `ravnd` — the Ravn agent daemon.
//!
//! Transport skeleton (#22): connect to the control plane (NATS, or a
//! WebSocket fallback) and publish a `Message`. Detection taps (epic #1),
//! local inference (epic #2), enrollment, heartbeat, and the offline buffer
//! (epic #3) hang off this. For now the daemon emits one startup event so the
//! agent → server → store thread is demonstrable end to end.

mod config;
mod transport;

use chrono::Utc;
use ravn_core::{AgentId, Event, JournaldPayload, Message, Payload, Severity};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use uuid::Uuid;

use crate::config::Config;
use crate::transport::Transport;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    init_tracing(&config);

    tracing::info!(
        agent_id = %config.agent_id,
        host = %config.host,
        server = %config.server_url,
        version = VERSION,
        "ravnd starting"
    );

    let transport = Transport::connect(&config.server_url, config.agent_id).await?;
    tracing::info!("transport connected");

    // Placeholder emission until detection taps land (#9). Modelled as a
    // journald line so it fits the schema honestly.
    let message = startup_message(&config);
    transport.publish(&message).await?;
    tracing::info!(event_id = %message.event.id, "published startup event");

    // Detection + periodic emission arrive later; idle until shutdown.
    shutdown_signal().await;
    Ok(())
}

/// Build the one-off startup event.
fn startup_message(config: &Config) -> Message {
    let now = Utc::now();
    Message::new(Event {
        id: Uuid::now_v7(),
        occurred_at: now,
        observed_at: now,
        agent_id: AgentId(config.agent_id),
        host: config.host.clone(),
        severity: Severity::Info,
        title: format!("ravnd {VERSION} started"),
        category_hints: Vec::new(),
        payload: Payload::Journald(JournaldPayload {
            unit: Some("ravnd.service".to_string()),
            priority: Some(6),
            message: format!("ravnd {VERSION} started on {}", config.host),
            ..Default::default()
        }),
    })
}

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log.clone()));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Resolve on SIGINT or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
