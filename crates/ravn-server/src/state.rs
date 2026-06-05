//! Shared application state handed to HTTP handlers and the ingestion task.

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::auth::{IngestAuth, TokenReviewValidator, UserAuth};
use crate::ca::Ca;
use crate::command::{CommandQueue, CommandSigner};
use crate::db::StoredEvent;
use crate::inference::InferenceClient;
use crate::metrics::Metrics;
use crate::remediation::{RemediationStore, TemplateRegistry};

/// Public OIDC settings the portal SPA needs to start the auth-code+PKCE flow
/// (#26). Served at `/auth/config`; contains no secrets.
#[derive(Clone, serde::Serialize)]
pub struct OidcPublic {
    pub issuer: String,
    pub client_id: String,
    pub scopes: String,
}

/// Cloneable handle to the control plane's runtime dependencies.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool.
    pub pool: PgPool,
    /// NATS client (cloneable; shares the underlying connection).
    pub nats: async_nats::Client,
    /// Fan-out of freshly ingested events to live WebSocket subscribers (#29).
    pub events_tx: broadcast::Sender<StoredEvent>,
    /// API auth tokens (#26). Auth is enforced only when at least one is set.
    pub admin_token: Option<String>,
    pub viewer_token: Option<String>,
    /// Prometheus metrics (#40).
    pub metrics: Arc<Metrics>,
    /// Internal CA for agent enrollment (#19). `None` disables `/enroll`.
    pub ca: Option<Arc<Ca>>,
    /// Bootstrap token agents present to enroll. Paired with `ca`.
    pub enroll_token: Option<String>,
    /// ServiceAccount-token validator for the authenticated HTTP ingest
    /// endpoint (#57). `None` disables JWKS validation.
    pub ingest_auth: Option<Arc<IngestAuth>>,
    /// Kubernetes TokenReview fallback for ingest auth (#102).
    pub ingest_token_review: Option<Arc<TokenReviewValidator>>,
    /// Shared inference client for async K8s-event explanations (#58). `None`
    /// disables explanation generation.
    pub inference: Option<Arc<InferenceClient>>,
    /// Portal user OIDC validator (#26). `None` disables user OIDC (static
    /// tokens still apply).
    pub user_auth: Option<Arc<UserAuth>>,
    /// Public OIDC settings served to the SPA at `/auth/config` (#26).
    pub oidc_public: Option<OidcPublic>,
    /// Ed25519 signer for remediation commands; its public key is delivered to
    /// agents at enrollment (#114).
    pub command_signer: Arc<CommandSigner>,
    /// Per-agent queue of signed commands awaiting pull, plus reported results
    /// (#114). The orchestrator (#115) enqueues; agents pull and report.
    pub command_queue: Arc<CommandQueue>,
    /// Curated remediation templates loaded at startup (#115).
    pub templates: Arc<TemplateRegistry>,
    /// In-memory remediation records: proposals, decisions, results (#115).
    pub remediations: Arc<RemediationStore>,
    /// How long (seconds) signed commands stay valid (#114/#115).
    pub command_ttl_secs: i64,
}
