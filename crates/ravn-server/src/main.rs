//! `ravn-server` — the Ravn control plane.
//!
//! Axum API skeleton (#23): config, structured tracing, liveness/readiness
//! probes, and a served OpenAPI document. NATS ingestion + Postgres
//! persistence (#24), the agent registry (#25), and auth (#26) hang off this.

mod api;
mod auth;
mod ca;
mod config;
mod db;
mod ingest;
mod metrics;
mod state;

use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::auth::IngestAuth;
use crate::config::{Config, IngestAuthConfig, JwksSource};
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

    let (events_tx, _) = tokio::sync::broadcast::channel(256);
    if config.admin_token.is_some() || config.viewer_token.is_some() {
        tracing::info!("API auth enabled (bearer token + RBAC)");
    } else {
        tracing::warn!("API auth disabled — set RAVN_ADMIN_TOKEN / RAVN_VIEWER_TOKEN to enable");
    }

    // Agent enrollment CA (#19): build from config when configured.
    let (ca, enroll_token) = match &config.enroll {
        Some(e) => {
            let ca = ca::Ca::load(&e.ca_cert_pem, &e.ca_key_pem, e.cert_ttl_days)
                .context("loading enrollment CA")?;
            tracing::info!(ttl_days = e.cert_ttl_days, "agent enrollment enabled (/enroll)");
            (Some(std::sync::Arc::new(ca)), Some(e.bootstrap_token.clone()))
        }
        None => {
            tracing::info!("agent enrollment disabled — set RAVN_ENROLL_TOKEN/RAVN_CA_CERT/RAVN_CA_KEY");
            (None, None)
        }
    };

    // Authenticated HTTP ingest (#57): load the cluster OIDC JWKS and build the
    // ServiceAccount-token validator when configured.
    let ingest_auth = match &config.ingest_auth {
        Some(cfg) => {
            let auth = build_ingest_auth(cfg).await.context("initializing ingest auth")?;
            tracing::info!(
                issuer = %cfg.issuer,
                audience = %cfg.audience,
                "authenticated HTTP ingest enabled (/ingest, ServiceAccount OIDC)"
            );
            Some(std::sync::Arc::new(auth))
        }
        None => {
            tracing::info!("authenticated HTTP ingest disabled — set RAVN_INGEST_OIDC_ISSUER + JWKS");
            None
        }
    };

    let app_state = AppState {
        pool,
        nats,
        events_tx,
        admin_token: config.admin_token.clone(),
        viewer_token: config.viewer_token.clone(),
        metrics: std::sync::Arc::new(metrics::Metrics::new()),
        ca,
        enroll_token,
        ingest_auth,
    };

    // Spawn the ingestion loops (events + heartbeats).
    tokio::spawn(ingest::run(app_state.clone()));
    tokio::spawn(ingest::run_heartbeats(app_state.clone()));

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

/// Load the OIDC JWKS (from a URL or a file) and build the ingest-token
/// validator. Fetched once at startup; rotating cluster signing keys requires
/// a restart (acceptable for M0 — keys rotate on the order of months).
async fn build_ingest_auth(cfg: &IngestAuthConfig) -> anyhow::Result<IngestAuth> {
    let jwks_json = match &cfg.jwks_source {
        JwksSource::Url(url) => reqwest::get(url)
            .await
            .with_context(|| format!("fetching JWKS from {url}"))?
            .error_for_status()
            .context("JWKS endpoint returned an error status")?
            .text()
            .await
            .context("reading JWKS response body")?,
        JwksSource::File(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading JWKS file {path}"))?,
    };
    IngestAuth::new(cfg.issuer.clone(), cfg.audience.clone(), &jwks_json)
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
