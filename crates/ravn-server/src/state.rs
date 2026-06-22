//! Shared application state handed to HTTP handlers and the ingestion task.

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::auth::{IngestAuth, TokenReviewValidator, UserAuth};
use crate::ca::Ca;
use crate::command::{CommandQueue, CommandSigner};
use crate::db::StoredEvent;
use crate::inference::InferenceClient;
use crate::knowledge::KnowledgeBase;
use crate::metrics::Metrics;
use crate::policy::PolicyEngine;
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
    /// Per-environment remediation knowledge base (#118): deterministic
    /// retrospective writing, recall, and gap tracking. Disabled when
    /// `RAVN_KB_DIR` is unset.
    pub knowledge: Arc<KnowledgeBase>,
    /// Declarative default-deny policy engine: decides auto vs. approve vs.
    /// forbid, with the circuit breaker and kill switch (#116).
    pub policy: Arc<PolicyEngine>,
    /// How long (seconds) signed commands stay valid (#114/#115).
    pub command_ttl_secs: i64,
    /// Token bucket rate-limiter for HTTP ingest.
    pub ingest_rate_limiter: Arc<std::sync::Mutex<TokenBucket>>,
}

/// A thread-safe token bucket rate-limiter.
pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_rate: f64,
    last_refill: std::time::Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            refill_rate,
            last_refill: std::time::Instant::now(),
        }
    }

    pub fn consume(&mut self, amount: f64) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;

        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);

        if self.tokens >= amount {
            self.tokens -= amount;
            true
        } else {
            false
        }
    }
}

