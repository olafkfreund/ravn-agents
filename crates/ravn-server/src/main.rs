//! `ravn-server` — the Ravn control plane.
//!
//! Axum API skeleton (#23): config, structured tracing, liveness/readiness
//! probes, and a served OpenAPI document. NATS ingestion + Postgres
//! persistence (#24), the agent registry (#25), and auth (#26) hang off this.

mod api;
mod config;
mod db;
mod ingest;
mod state;

use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::Config;
use crate::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(&config);

    // Database: connect and migrate before serving.
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("database connected and migrated");

    // NATS: connect for ingestion.
    let nats = async_nats::connect(&config.nats_url)
        .await
        .with_context(|| format!("connecting to NATS at {}", config.nats_url))?;
    tracing::info!(url = %config.nats_url, "NATS connected");

    let app_state = AppState { pool, nats };

    // Spawn the ingestion loop (NATS -> validate -> persist).
    tokio::spawn(ingest::run(app_state.clone()));

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;

    tracing::info!(addr = %config.bind, version = VERSION, "ravn-server listening");

    axum::serve(listener, api::router(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

/// Initialize structured logging. `RUST_LOG` wins; otherwise the configured
/// default filter applies.
fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log.clone()));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Resolve when the process receives SIGINT or SIGTERM, so axum can drain.
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

    tracing::info!("shutdown signal received, draining");
}
