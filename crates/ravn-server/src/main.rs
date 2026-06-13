//! `ravn-server` — the Ravn control plane.
//!
//! Axum API skeleton (#23): config, structured tracing, liveness/readiness
//! probes, and a served OpenAPI document. NATS ingestion + Postgres
//! persistence (#24), the agent registry (#25), and auth (#26) hang off this.

mod api;
mod auth;
mod ca;
mod command;
mod config;
mod db;
mod inference;
mod ingest;
mod knowledge;
mod metrics;
mod policy;
mod remediation;
mod state;

use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::auth::{IngestAuth, TokenReviewValidator, UserAuth};
use crate::config::{Config, IngestAuthConfig, JwksSource, OidcConfig};
use crate::state::{AppState, OidcPublic};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(&config);

    if config::is_airgapped() {
        tracing::info!(
            "RAVN_AIRGAPPED=1 — air-gapped mode active: \
             all JWKS documents must be local files; outbound JWKS URL fetches are blocked"
        );
    }

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

    // Shared inference endpoint for async K8s-event explanations (#58).
    let inference = match &config.inference {
        Some(cfg) => {
            tracing::info!(
                endpoint = %cfg.endpoint,
                model = %cfg.model,
                "K8s-event inference enabled (async explanations)"
            );
            Some(std::sync::Arc::new(inference::InferenceClient::new(
                cfg.endpoint.clone(),
                cfg.model.clone(),
                cfg.api_key.clone(),
                std::time::Duration::from_secs(cfg.timeout_secs),
            )))
        }
        None => {
            tracing::info!("K8s-event inference disabled — set RAVN_INFERENCE_ENDPOINT");
            None
        }
    };

    // Portal user OIDC + RBAC (#26).
    let (user_auth, oidc_public) = match &config.oidc {
        Some(cfg) => {
            let ua = build_user_auth(cfg).await.context("initializing user OIDC auth")?;
            let client_id = cfg.client_id.clone();
            tracing::info!(
                issuer = %cfg.issuer,
                admin_group = cfg.admin_group.as_deref().unwrap_or("<none>"),
                "portal user OIDC enabled (bearer JWT + RBAC)"
            );
            let public = OidcPublic { issuer: cfg.issuer.clone(), client_id, scopes: cfg.scopes.clone() };
            (Some(std::sync::Arc::new(ua)), Some(public))
        }
        None => {
            tracing::info!("portal user OIDC disabled — set RAVN_OIDC_ISSUER + JWKS");
            (None, None)
        }
    };

    // Kubernetes TokenReview fallback for ingest auth (#102).
    let ingest_token_review = match &config.ingest_token_review {
        Some(cfg) => {
            let v = TokenReviewValidator::new(
                &cfg.api_url,
                cfg.bearer.clone(),
                cfg.ca_pem.as_deref(),
                cfg.audience.clone(),
            )
            .context("initializing TokenReview validator")?;
            tracing::info!(api = %cfg.api_url, "ingest TokenReview fallback enabled");
            Some(std::sync::Arc::new(v))
        }
        None => None,
    };

    // Remediation command signing key (#114): its public key is advertised to
    // agents at enrollment; the orchestrator (#115) signs commands with it.
    let command_signer = std::sync::Arc::new(
        command::CommandSigner::load_or_generate(config.command_key_path.as_deref())
            .context("initializing the remediation command signing key")?,
    );
    let command_queue = std::sync::Arc::new(command::CommandQueue::default());

    // Curated remediation templates (#115): loaded once at startup.
    let templates = std::sync::Arc::new(
        remediation::TemplateRegistry::load_dir(std::path::Path::new(&config.templates_dir))
            .context("loading remediation templates")?,
    );
    let remediations = std::sync::Arc::new(remediation::RemediationStore::default());

    // Remediation knowledge base (#118): per-env markdown wiki for deterministic
    // recall + gap tracking. Disabled (no-op) when RAVN_KB_DIR is unset.
    let knowledge = std::sync::Arc::new(
        knowledge::KnowledgeBase::open(config.kb_dir.as_deref())
            .context("opening the remediation knowledge base")?,
    );

    // Remediation policy engine (#116): the per-env default-deny policy plus the
    // global auto kill switch and the circuit breaker. With no policy file and
    // the kill switch off (the defaults), every match requires approval (P1).
    let policy_doc = match &config.policy.file {
        Some(path) => policy::Policy::load_file(std::path::Path::new(path))
            .context("loading remediation policy")?,
        None => {
            tracing::info!("no RAVN_POLICY_FILE set — all remediations require approval (default-deny)");
            policy::Policy::default()
        }
    };
    if config.policy.auto_enabled {
        tracing::info!(
            max = config.policy.auto_max,
            window_secs = config.policy.auto_window_secs,
            "remediation auto-execute ENABLED (policy-gated, circuit-broken)"
        );
    } else {
        tracing::info!("remediation auto-execute disabled (kill switch) — set RAVN_REMEDIATION_AUTO=1 to enable");
    }
    let policy = std::sync::Arc::new(policy::PolicyEngine::new(
        policy_doc,
        config.policy.auto_enabled,
        config.policy.auto_max,
        std::time::Duration::from_secs(config.policy.auto_window_secs),
    ));

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
        ingest_token_review,
        inference,
        user_auth,
        oidc_public,
        command_signer,
        command_queue,
        templates,
        remediations,
        knowledge,
        policy,
        command_ttl_secs: config.command_ttl_secs,
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
    let jwks_json = load_jwks(&cfg.jwks_source).await?;
    IngestAuth::new(cfg.issuer.clone(), cfg.audience.clone(), &jwks_json)
}

/// Build the portal user OIDC validator from config (#26).
async fn build_user_auth(cfg: &OidcConfig) -> anyhow::Result<UserAuth> {
    let jwks_json = load_jwks(&cfg.jwks_source).await?;
    UserAuth::new(
        cfg.issuer.clone(),
        Some(cfg.audience.clone()),
        &jwks_json,
        cfg.groups_claim.clone(),
        cfg.admin_group.clone(),
        cfg.viewer_group.clone(),
    )
}

/// Load a JWKS document from a URL (fetched at startup) or a local file.
///
/// When `RAVN_AIRGAPPED=1` is set the URL variant is rejected at startup rather
/// than silently making an outbound HTTPS call — the operator must supply the
/// JWKS as a local file (`RAVN_OIDC_JWKS_FILE` / `RAVN_INGEST_OIDC_JWKS_FILE`).
async fn load_jwks(source: &JwksSource) -> anyhow::Result<String> {
    if crate::config::is_airgapped() {
        if let JwksSource::Url(url) = source {
            anyhow::bail!(
                "RAVN_AIRGAPPED=1 is set but JWKS source is a URL ({url}). \
                 In air-gapped mode all JWKS documents must be provided as local \
                 files (RAVN_OIDC_JWKS_FILE / RAVN_INGEST_OIDC_JWKS_FILE)."
            );
        }
    }
    match source {
        JwksSource::Url(url) => reqwest::get(url)
            .await
            .with_context(|| format!("fetching JWKS from {url}"))?
            .error_for_status()
            .context("JWKS endpoint returned an error status")?
            .text()
            .await
            .context("reading JWKS response body"),
        JwksSource::File(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading JWKS file {path}"))
        }
    }
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
