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
mod enrollment;
mod remediation;
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

    // Enrollment (#19): obtain/reuse the agent's mTLS identity before
    // connecting. Best-effort — a failure must not stop detection (the mTLS
    // handshake on the transport itself is wired in #26).
    match enrollment::ensure_enrolled(&config).await {
        Ok(Some(_)) => {}
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "enrollment failed; continuing without an mTLS identity"),
    }

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

    // Remediation command pull (#114): opt-in. Pull signed commands over the
    // existing outbound HTTP path, verify against the pinned key, and relay to
    // the privileged actuator. ravnd never executes a capability itself.
    if config.remediation_enable {
        spawn_remediation(&config);
    }

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

/// Spawn the remediation command-pull loop (#114) when a pinned key and an HTTP
/// control-plane endpoint are available; otherwise log why it stays off.
fn spawn_remediation(config: &Config) {
    let Some(key) = remediation::load_pinned_key(&config.cred_dir) else {
        tracing::warn!("remediation enabled but no pinned command key (enroll first); pull disabled");
        return;
    };
    let Some(base) = config.enroll_endpoint.clone() else {
        tracing::warn!(
            "remediation enabled but RAVN_ENROLL_ENDPOINT (HTTP control plane) is unset; pull disabled"
        );
        return;
    };
    let ledger = Arc::new(remediation::Ledger::load(config.cred_dir.join("command-ledger")));
    tracing::info!(socket = %config.actuator_socket.display(), "remediation command pull enabled");
    tokio::spawn(remediation::run(
        reqwest::Client::new(),
        base,
        config.agent_id,
        config.api_token.clone(),
        key,
        ledger,
        config.actuator_socket.clone(),
        config.command_poll_secs,
    ));
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
            skip_kernel: config.journald_skip_kernel,
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
        Arc::new(inference::InferenceClient::new(
            config.inference_endpoint.clone(),
            config.inference_model.clone(),
            std::time::Duration::from_secs(config.inference_timeout_secs),
        ))
    });

    // Digest mode (#17): instead of explaining each event inline, collect a
    // window and summarize it with one batched inference call. Bounds CPU and
    // hides per-event latency. Per-event enrichment is skipped while it's on.
    let digest_buf: Option<Arc<Mutex<Vec<Event>>>> = match (&inference, config.digest_enable) {
        (Some(client), true) => {
            let buf = Arc::new(Mutex::new(Vec::<Event>::new()));
            spawn_digest(
                config.digest_interval_secs,
                config.agent_id,
                config.host.clone(),
                transport.clone(),
                buffer.clone(),
                client.clone(),
                buf.clone(),
                health.clone(),
            );
            tracing::info!(
                interval_secs = config.digest_interval_secs,
                "digest mode enabled; per-event inference disabled"
            );
            Some(buf)
        }
        _ => None,
    };

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            maybe = rx.recv() => match maybe {
                Some(mut message) => {
                    if let Some(buf) = &digest_buf {
                        // Collect for the next digest (scoped by severity); the
                        // event itself still publishes immediately, bare.
                        if message.event.severity >= config.digest_min_severity {
                            let mut g = buf.lock().unwrap();
                            if g.len() < config.digest_max_events {
                                g.push(message.event.clone());
                            }
                        }
                    } else if let Some(client) = &inference {
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

/// Periodically drain the accumulated window into a single batched-inference
/// digest and publish it (#17).
#[allow(clippy::too_many_arguments)]
fn spawn_digest(
    interval_secs: u64,
    agent_id: Uuid,
    host: String,
    transport: Arc<Transport>,
    buffer: Option<Arc<buffer::Buffer>>,
    client: Arc<inference::InferenceClient>,
    accumulator: Arc<Mutex<Vec<Event>>>,
    health: Health,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
        tick.tick().await; // the first tick fires immediately; skip it
        loop {
            tick.tick().await;
            let batch: Vec<Event> = {
                let mut g = accumulator.lock().unwrap();
                std::mem::take(&mut *g)
            };
            if batch.is_empty() {
                continue;
            }

            let explanation = client.digest(&batch).await;
            let message = build_digest_message(agent_id, &host, &batch, explanation);
            match transport.publish(&message).await {
                Ok(()) => {
                    health.published.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(events = batch.len(), "published digest");
                }
                Err(error) => match &buffer {
                    Some(b) => {
                        let _ = b.enqueue(&message);
                        tracing::warn!(%error, "digest publish failed; buffered");
                    }
                    None => tracing::warn!(%error, "digest publish failed (no buffer)"),
                },
            }
        }
    });
}

/// Build the digest message wrapping a window of events plus the batched
/// explanation. Represented as a journald-style event (`ravn-digest`) so it
/// rides the existing schema; its severity is the window's maximum.
fn build_digest_message(
    agent_id: Uuid,
    host: &str,
    events: &[Event],
    explanation: Option<ravn_core::Explanation>,
) -> Message {
    let now = Utc::now();
    let severity = events.iter().map(|e| e.severity).max().unwrap_or(Severity::Notice);

    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in events {
        *counts.entry(format!("{:?}", e.source())).or_default() += 1;
    }
    let summary = counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");

    let mut message = Message::new(Event {
        id: Uuid::now_v7(),
        occurred_at: now,
        observed_at: now,
        agent_id: AgentId(agent_id),
        host: host.to_string(),
        severity,
        title: format!("Digest: {} events ({summary})", events.len()),
        category_hints: vec!["digest".to_string()],
        payload: Payload::Journald(JournaldPayload {
            unit: Some("ravn-digest".to_string()),
            priority: None,
            message: format!("Periodic digest of {} events — {summary}", events.len()),
            ..Default::default()
        }),
    });
    message.explanation = explanation;
    message
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

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::{Explanation, FailedUnitPayload, JournaldPayload, Source};

    fn ev(severity: Severity, payload: Payload) -> Event {
        let now = Utc::now();
        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::now_v7()),
            host: "host-9".into(),
            severity,
            title: "t".into(),
            category_hints: vec![],
            payload,
        }
    }

    #[test]
    fn digest_message_summarizes_window() {
        let events = vec![
            ev(Severity::Warning, Payload::Journald(JournaldPayload { message: "a".into(), ..Default::default() })),
            ev(Severity::Error, Payload::FailedUnit(FailedUnitPayload { unit: "x".into(), result: "y".into(), ..Default::default() })),
        ];
        let expl = Some(Explanation {
            text: "two issues; the failed unit matters most".into(),
            suggested_check: Some("systemctl --failed".into()),
            model: "m".into(),
            generated_at: Utc::now(),
        });
        let msg = build_digest_message(Uuid::now_v7(), "host-9", &events, expl);

        assert_eq!(msg.event.severity, Severity::Error, "digest takes the window's max severity");
        assert!(msg.event.title.contains("2 events"));
        assert!(msg.event.category_hints.contains(&"digest".to_string()));
        // Rides the existing journald schema (no new payload variant).
        assert_eq!(msg.event.source(), Source::Journald);
        assert_eq!(msg.explanation.as_ref().unwrap().text, "two issues; the failed unit matters most");
    }

    #[test]
    fn empty_window_defaults_to_notice() {
        let msg = build_digest_message(Uuid::now_v7(), "h", &[], None);
        assert_eq!(msg.event.severity, Severity::Notice);
        assert!(msg.explanation.is_none());
    }
}
