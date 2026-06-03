//! `ravnd` — the Ravn agent daemon.
//!
//! Transport skeleton (#22): connect to the control plane (NATS, or a
//! WebSocket fallback) and publish a `Message`. Detection taps (epic #1),
//! local inference (epic #2), enrollment, heartbeat, and the offline buffer
//! (epic #3) hang off this. For now the daemon emits one startup event so the
//! agent → server → store thread is demonstrable end to end.

mod buffer;
mod config;
mod detection;
mod transport;

use ravn_agent::inference;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ravn_core::{AgentId, Event, Heartbeat, JournaldPayload, Message, Payload, Severity};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use uuid::Uuid;

use crate::config::Config;
use crate::transport::Transport;

/// Health state shared between the detection loop and the heartbeat task.
#[derive(Clone)]
struct Health {
    started: Instant,
    last_detection: Arc<Mutex<Option<DateTime<Utc>>>>,
    published: Arc<AtomicU64>,
}

impl Health {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            last_detection: Arc::new(Mutex::new(None)),
            published: Arc::new(AtomicU64::new(0)),
        }
    }
}

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

    let transport = Arc::new(Transport::connect(&config.server_url, config.agent_id).await?);
    tracing::info!("transport connected");

    let health = Health::new();

    // Local offline buffer: durably queue events when the control plane is
    // unreachable; a flush task drains it on reconnect (#21).
    let buffer = match buffer::Buffer::open(&config.buffer_path, config.buffer_max) {
        Ok(b) => {
            if let Ok(n) = b.len() {
                if n > 0 {
                    tracing::info!(buffered = n, "offline buffer has pending events");
                }
            }
            Some(Arc::new(b))
        }
        Err(error) => {
            tracing::warn!(%error, path = %config.buffer_path, "offline buffer disabled");
            None
        }
    };

    // Announce the agent is online (a journald-style "ravnd started" line).
    let startup = startup_message(&config);
    if let Err(error) = transport.publish(&startup).await {
        tracing::warn!(%error, "buffering startup event");
        if let Some(b) = &buffer {
            let _ = b.enqueue(&startup);
        }
    } else {
        tracing::info!(event_id = %startup.event.id, "published startup event");
    }

    // Heartbeat task: keeps the control plane's liveness fresh even when quiet.
    spawn_heartbeat(&config, transport.clone(), health.clone());

    // Flush task: drain the offline buffer to the control plane.
    if let Some(b) = &buffer {
        spawn_flush(transport.clone(), b.clone());
    }

    let any_tap = config.journald_enable
        || !config.config_drift_paths.is_empty()
        || config.failed_units_enable
        || config.updates_enable;
    if any_tap {
        run_detection(&config, transport, health, buffer).await;
    } else {
        tracing::info!("no detection taps enabled; idling (heartbeat only)");
        shutdown_signal().await;
    }

    Ok(())
}

/// Periodically publish a heartbeat with agent health.
fn spawn_heartbeat(config: &Config, transport: Arc<Transport>, health: Health) {
    let agent_id = config.agent_id;
    let host = config.host.clone();
    let inference_enabled = config.inference_enable;
    let interval = Duration::from_secs(config.heartbeat_interval_secs);

    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            let hb = Heartbeat {
                agent_id: AgentId(agent_id),
                host: host.clone(),
                sent_at: Utc::now(),
                uptime_secs: health.started.elapsed().as_secs(),
                last_detection: *health.last_detection.lock().unwrap(),
                inference_enabled,
                events_published: health.published.load(Ordering::Relaxed),
            };
            if let Err(error) = transport.publish_heartbeat(&hb).await {
                tracing::debug!(%error, "heartbeat publish failed");
            }
        }
    });
}

/// Periodically drain the offline buffer to the control plane.
fn spawn_flush(transport: Arc<Transport>, buffer: Arc<buffer::Buffer>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        loop {
            tick.tick().await;
            match buffer.drain_batch(100) {
                Ok(batch) if !batch.is_empty() => {
                    for (id, message) in batch {
                        match transport.publish(&message).await {
                            Ok(()) => {
                                let _ = buffer.delete(&id);
                            }
                            Err(_) => break, // still unreachable; retry next tick
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "buffer drain failed"),
            }
        }
    });
}

/// Run the detection taps, publishing each emitted event until shutdown.
async fn run_detection(
    config: &Config,
    transport: Arc<Transport>,
    health: Health,
    buffer: Option<Arc<buffer::Buffer>>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(256);

    if config.journald_enable {
        let tap = detection::journald::JournaldTap {
            agent_id: config.agent_id,
            host: config.host.clone(),
            min_priority: config.journald_min_priority,
            auth_enable: config.auth_enable,
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(error) = tap.run(tx).await {
                tracing::error!(%error, "journald tap exited");
            }
        });
    }

    if !config.config_drift_paths.is_empty() {
        let tap = detection::config_drift::ConfigDriftTap {
            agent_id: config.agent_id,
            host: config.host.clone(),
            paths: config.config_drift_paths.clone(),
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(error) = tap.run(tx).await {
                tracing::error!(%error, "config-drift tap exited");
            }
        });
    }

    if config.failed_units_enable {
        let tap = detection::failed_unit::FailedUnitTap {
            agent_id: config.agent_id,
            host: config.host.clone(),
            user_bus: config.systemd_user_bus,
            poll_interval: std::time::Duration::from_secs(5),
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(error) = tap.run(tx).await {
                tracing::error!(%error, "failed-unit tap exited");
            }
        });
    }

    if config.updates_enable {
        let tap = detection::update::UpdateTap {
            agent_id: config.agent_id,
            host: config.host.clone(),
            profile: config.nix_profile.clone(),
            poll_interval: std::time::Duration::from_secs(config.update_poll_secs),
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(error) = tap.run(tx).await {
                tracing::error!(%error, "update tap exited");
            }
        });
    }

    // Close the original sender so the loop ends once all taps stop.
    drop(tx);

    // Optional local inference: enrich events with an explanation before
    // publishing. Best-effort with a timeout — a slow/down model never blocks
    // the alarm, only its wording.
    let inference = config.inference_enable.then(|| {
        tracing::info!(endpoint = %config.inference_endpoint, "inference enrichment enabled");
        inference::InferenceClient::new(
            config.inference_endpoint.clone(),
            config.inference_model.clone(),
            std::time::Duration::from_secs(config.inference_timeout_secs),
        )
    });

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            maybe = rx.recv() => match maybe {
                Some(mut message) => {
                    if let Some(client) = &inference {
                        if message.explanation.is_none() {
                            message.explanation = client.explain(&message.event).await;
                        }
                    }
                    match transport.publish(&message).await {
                        Ok(()) => {
                            health.published.fetch_add(1, Ordering::Relaxed);
                            *health.last_detection.lock().unwrap() = Some(Utc::now());
                        }
                        Err(error) => {
                            // Buffer for later instead of dropping the alarm.
                            match &buffer {
                                Some(b) => {
                                    if let Err(e) = b.enqueue(&message) {
                                        tracing::warn!(error = %e, "failed to buffer event");
                                    } else {
                                        tracing::warn!(%error, "publish failed; buffered event");
                                    }
                                }
                                None => tracing::warn!(%error, "failed to publish event (no buffer)"),
                            }
                        }
                    }
                }
                None => break, // tap ended
            },
        }
    }

    tracing::info!(published = health.published.load(Ordering::Relaxed), "detection loop ended");
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
