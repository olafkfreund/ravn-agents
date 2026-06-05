//! Runtime configuration for the control plane, sourced from the environment.

use std::net::SocketAddr;

use anyhow::Context;

/// Control-plane configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP API binds to. `RAVN_BIND`, default `0.0.0.0:8080`.
    pub bind: SocketAddr,
    /// Default tracing filter when `RUST_LOG` is unset. `RAVN_LOG`, default `info`.
    pub log: String,
    /// PostgreSQL connection string. `DATABASE_URL`.
    pub database_url: String,
    /// NATS server URL. `NATS_URL`, default `nats://127.0.0.1:4222`.
    pub nats_url: String,
    /// Bearer token granting full (admin) API access. `RAVN_ADMIN_TOKEN`.
    pub admin_token: Option<String>,
    /// Bearer token granting read-only API access. `RAVN_VIEWER_TOKEN`.
    pub viewer_token: Option<String>,
    /// Agent enrollment config (#19). `Some` only when the bootstrap token and
    /// CA cert/key are all provided.
    pub enroll: Option<EnrollConfig>,
    /// Authenticated HTTP ingest config (#57). `Some` enables `/ingest` with
    /// ServiceAccount-token (OIDC/JWKS) validation.
    pub ingest_auth: Option<IngestAuthConfig>,
    /// Kubernetes TokenReview fallback for ingest auth (#102). `Some` lets the
    /// control plane validate agent SA tokens via the cluster API when JWKS
    /// validation is unavailable.
    pub ingest_token_review: Option<TokenReviewConfig>,
    /// Shared inference endpoint for explaining K8s events (#58). `Some`
    /// enables async explanation generation.
    pub inference: Option<InferenceConfig>,
    /// Portal user authentication via OIDC + RBAC (#26). `Some` enables
    /// OIDC-bearer validation on the API (alongside the static dev tokens).
    pub oidc: Option<OidcConfig>,
    /// Path to the Ed25519 remediation command signing key (#114).
    /// `RAVN_COMMAND_KEY`. Generated and persisted (`0600`) if absent; when
    /// unset entirely, an ephemeral key is used (dev only).
    pub command_key_path: Option<String>,
    /// Directory of curated remediation templates (#115). `RAVN_TEMPLATES_DIR`,
    /// default `templates`. Missing dir → no proposals.
    pub templates_dir: String,
    /// How long (seconds) a signed command stays valid. `RAVN_COMMAND_TTL_SECS`,
    /// default 300.
    pub command_ttl_secs: i64,
    /// Remediation policy engine settings (#116): the per-env policy file plus
    /// the global auto kill switch and circuit-breaker limits.
    pub policy: PolicyConfig,
}

/// Remediation policy-engine configuration (#116). When no policy file is set,
/// the default (empty) policy makes every match `approve` — P1 behaviour.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Path to the per-env policy TOML. `RAVN_POLICY_FILE`. Missing file → an
    /// empty (default-deny) policy where everything requires approval.
    pub file: Option<String>,
    /// Fleet-wide kill switch. `RAVN_REMEDIATION_AUTO`, default **off**: when
    /// off, every decision is forced to `approve` regardless of policy.
    pub auto_enabled: bool,
    /// Circuit-breaker cap: max auto-executions per `(host, template)` within
    /// the window. `RAVN_REMEDIATION_AUTO_MAX`, default 5.
    pub auto_max: u32,
    /// Circuit-breaker window in seconds. `RAVN_REMEDIATION_AUTO_WINDOW_SECS`,
    /// default 300.
    pub auto_window_secs: u64,
}

/// Portal user OIDC + RBAC configuration (#26).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// OIDC issuer URL. `RAVN_OIDC_ISSUER` (enables user auth).
    pub issuer: String,
    /// Where to load the IdP JWKS from. `RAVN_OIDC_JWKS_URL` / `_FILE`.
    pub jwks_source: JwksSource,
    /// Expected token audience / OIDC client id. `RAVN_OIDC_AUDIENCE`
    /// (required when OIDC is enabled — audience validation is never skipped).
    pub audience: String,
    /// Public client id the SPA uses for the auth-code+PKCE flow.
    /// `RAVN_OIDC_CLIENT_ID` (defaults to `audience`).
    pub client_id: String,
    /// Claim holding group memberships. `RAVN_OIDC_GROUPS_CLAIM`, default `groups`.
    pub groups_claim: String,
    /// Group granting admin. `RAVN_OIDC_ADMIN_GROUP`.
    pub admin_group: Option<String>,
    /// Group required for viewer access. `RAVN_OIDC_VIEWER_GROUP` (optional;
    /// when unset, any authenticated user is a viewer).
    pub viewer_group: Option<String>,
    /// OAuth scopes the SPA requests. `RAVN_OIDC_SCOPES`,
    /// default `openid profile email groups`.
    pub scopes: String,
}

/// Configuration for the shared inference endpoint (#58).
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    /// OpenAI-compatible base URL. `RAVN_INFERENCE_ENDPOINT`
    /// (e.g. `http://inference.svc/v1`).
    pub endpoint: String,
    /// Model name to request. `RAVN_INFERENCE_MODEL`, default `default`.
    pub model: String,
    /// Optional bearer API key. `RAVN_INFERENCE_API_KEY` / `_FILE`.
    pub api_key: Option<String>,
    /// Request timeout in seconds. `RAVN_INFERENCE_TIMEOUT_SECS`, default 30.
    pub timeout_secs: u64,
}

/// Configuration for the authenticated HTTP ingest endpoint (#57).
#[derive(Debug, Clone)]
pub struct IngestAuthConfig {
    /// Expected token issuer (the cluster's OIDC issuer).
    /// `RAVN_INGEST_OIDC_ISSUER`.
    pub issuer: String,
    /// Expected audience on presented tokens. `RAVN_INGEST_AUDIENCE`,
    /// default `ravn`.
    pub audience: String,
    /// Where to load the OIDC JWKS document from: a URL fetched at startup
    /// (`RAVN_INGEST_OIDC_JWKS_URL`) or a local file
    /// (`RAVN_INGEST_OIDC_JWKS_FILE`).
    pub jwks_source: JwksSource,
}

/// Kubernetes TokenReview fallback for ingest auth (#102).
#[derive(Debug, Clone)]
pub struct TokenReviewConfig {
    /// Cluster API base URL. `RAVN_INGEST_TOKENREVIEW_URL`
    /// (`/apis/authentication.k8s.io/v1/tokenreviews` is appended).
    pub api_url: String,
    /// Bearer the control plane authenticates to the API with (needs
    /// `system:auth-delegator`). `RAVN_INGEST_TOKENREVIEW_TOKEN[_FILE]`.
    pub bearer: String,
    /// Optional PEM CA bundle for the cluster API TLS.
    /// `RAVN_INGEST_TOKENREVIEW_CA_FILE`.
    pub ca_pem: Option<String>,
    /// Audience the presented token must be valid for. `RAVN_INGEST_AUDIENCE`,
    /// default `ravn`.
    pub audience: String,
}

/// Source of the OIDC JWKS document.
#[derive(Debug, Clone)]
pub enum JwksSource {
    /// Fetch over HTTPS at startup.
    Url(String),
    /// Read from a local file (e.g. a mounted ConfigMap).
    File(String),
}

/// Configuration for the bootstrap-token → mTLS enrollment endpoint (#19).
#[derive(Debug, Clone)]
pub struct EnrollConfig {
    /// Shared bootstrap token agents present to enroll. `RAVN_ENROLL_TOKEN`.
    pub bootstrap_token: String,
    /// PEM CA certificate. Read from the path in `RAVN_CA_CERT`.
    pub ca_cert_pem: String,
    /// PEM CA private key. Read from the path in `RAVN_CA_KEY`.
    pub ca_key_pem: String,
    /// Validity of issued certificates, in days. `RAVN_CERT_TTL_DAYS`, default 90.
    pub cert_ttl_days: i64,
}

impl Config {
    /// Build configuration from environment variables, applying defaults.
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("RAVN_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let bind: SocketAddr = bind
            .parse()
            .with_context(|| format!("RAVN_BIND is not a valid socket address: {bind:?}"))?;

        let log = std::env::var("RAVN_LOG").unwrap_or_else(|_| "info".to_string());

        let database_url = std::env::var("DATABASE_URL")
            .context("DATABASE_URL must be set (PostgreSQL connection string)")?;

        let nats_url =
            std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());

        let token = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let admin_token = token("RAVN_ADMIN_TOKEN");
        let viewer_token = token("RAVN_VIEWER_TOKEN");

        let enroll = Self::enroll_from_env(&token)?;
        let ingest_auth = Self::ingest_auth_from_env(&token)?;
        let ingest_token_review = Self::token_review_from_env(&token)?;
        let inference = Self::inference_from_env(&token)?;
        let oidc = Self::oidc_from_env(&token)?;
        let command_key_path = token("RAVN_COMMAND_KEY");
        let templates_dir = token("RAVN_TEMPLATES_DIR").unwrap_or_else(|| "templates".to_string());
        let command_ttl_secs = std::env::var("RAVN_COMMAND_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300)
            .max(1);

        let policy = PolicyConfig {
            file: token("RAVN_POLICY_FILE"),
            // Off unless explicitly enabled — the kill switch defaults closed.
            auto_enabled: matches!(
                std::env::var("RAVN_REMEDIATION_AUTO").ok().as_deref(),
                Some("1" | "true" | "yes" | "on")
            ),
            auto_max: std::env::var("RAVN_REMEDIATION_AUTO_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5)
                .max(1),
            auto_window_secs: std::env::var("RAVN_REMEDIATION_AUTO_WINDOW_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300)
                .max(1),
        };

        Ok(Self {
            bind,
            log,
            database_url,
            nats_url,
            admin_token,
            viewer_token,
            enroll,
            ingest_auth,
            ingest_token_review,
            inference,
            oidc,
            command_key_path,
            templates_dir,
            command_ttl_secs,
            policy,
        })
    }

    /// Assemble the TokenReview fallback config; `None` when no API URL is set.
    fn token_review_from_env(
        token: &impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Option<TokenReviewConfig>> {
        let Some(api_url) = token("RAVN_INGEST_TOKENREVIEW_URL") else {
            return Ok(None);
        };
        let bearer = match token("RAVN_INGEST_TOKENREVIEW_TOKEN") {
            Some(t) => t,
            None => match token("RAVN_INGEST_TOKENREVIEW_TOKEN_FILE") {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("reading RAVN_INGEST_TOKENREVIEW_TOKEN_FILE at {path}"))?
                    .trim()
                    .to_string(),
                None => anyhow::bail!(
                    "RAVN_INGEST_TOKENREVIEW_URL set but no credential — set RAVN_INGEST_TOKENREVIEW_TOKEN or _TOKEN_FILE"
                ),
            },
        };
        let ca_pem = match token("RAVN_INGEST_TOKENREVIEW_CA_FILE") {
            Some(path) => Some(
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading RAVN_INGEST_TOKENREVIEW_CA_FILE at {path}"))?,
            ),
            None => None,
        };
        let audience = token("RAVN_INGEST_AUDIENCE").unwrap_or_else(|| "ravn".to_string());
        Ok(Some(TokenReviewConfig { api_url, bearer, ca_pem, audience }))
    }

    /// Assemble portal OIDC config; `None` (disabled) when no issuer is set.
    fn oidc_from_env(token: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<OidcConfig>> {
        let Some(issuer) = token("RAVN_OIDC_ISSUER") else {
            return Ok(None);
        };
        let jwks_source = match (token("RAVN_OIDC_JWKS_URL"), token("RAVN_OIDC_JWKS_FILE")) {
            (Some(url), None) => JwksSource::Url(url),
            (None, Some(path)) => JwksSource::File(path),
            (Some(_), Some(_)) => {
                anyhow::bail!("set only one of RAVN_OIDC_JWKS_URL or RAVN_OIDC_JWKS_FILE")
            }
            (None, None) => anyhow::bail!(
                "RAVN_OIDC_ISSUER set but no JWKS source — set RAVN_OIDC_JWKS_URL or RAVN_OIDC_JWKS_FILE"
            ),
        };
        // Audience is mandatory: skipping it would let a token minted for a
        // different relying party at the same issuer authenticate here
        // (audience confusion). Fail closed rather than silently disable it.
        let audience = token("RAVN_OIDC_AUDIENCE").context(
            "RAVN_OIDC_ISSUER is set but RAVN_OIDC_AUDIENCE is not — set the expected token \
             audience (the OIDC client id); audience validation must not be skipped",
        )?;
        let client_id = token("RAVN_OIDC_CLIENT_ID").unwrap_or_else(|| audience.clone());
        let groups_claim = token("RAVN_OIDC_GROUPS_CLAIM").unwrap_or_else(|| "groups".to_string());
        let admin_group = token("RAVN_OIDC_ADMIN_GROUP");
        let viewer_group = token("RAVN_OIDC_VIEWER_GROUP");
        let scopes =
            token("RAVN_OIDC_SCOPES").unwrap_or_else(|| "openid profile email groups".to_string());
        Ok(Some(OidcConfig {
            issuer,
            jwks_source,
            audience,
            client_id,
            groups_claim,
            admin_group,
            viewer_group,
            scopes,
        }))
    }

    /// Assemble inference config; `None` (disabled) when no endpoint is set.
    fn inference_from_env(
        token: &impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Option<InferenceConfig>> {
        let Some(endpoint) = token("RAVN_INFERENCE_ENDPOINT") else {
            return Ok(None);
        };
        let model = token("RAVN_INFERENCE_MODEL").unwrap_or_else(|| "default".to_string());
        let api_key = match token("RAVN_INFERENCE_API_KEY") {
            Some(k) => Some(k),
            None => match token("RAVN_INFERENCE_API_KEY_FILE") {
                Some(path) => Some(
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("reading RAVN_INFERENCE_API_KEY_FILE at {path}"))?
                        .trim()
                        .to_string(),
                ),
                None => None,
            },
        };
        let timeout_secs = std::env::var("RAVN_INFERENCE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        Ok(Some(InferenceConfig { endpoint, model, api_key, timeout_secs }))
    }

    /// Assemble ingest-auth config. Enabled only when the issuer *and* a JWKS
    /// source are set; an issuer with no JWKS source is a misconfiguration.
    fn ingest_auth_from_env(
        token: &impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Option<IngestAuthConfig>> {
        let issuer = token("RAVN_INGEST_OIDC_ISSUER");
        let jwks_source = match (token("RAVN_INGEST_OIDC_JWKS_URL"), token("RAVN_INGEST_OIDC_JWKS_FILE")) {
            (Some(url), None) => Some(JwksSource::Url(url)),
            (None, Some(path)) => Some(JwksSource::File(path)),
            (Some(_), Some(_)) => {
                anyhow::bail!("set only one of RAVN_INGEST_OIDC_JWKS_URL or RAVN_INGEST_OIDC_JWKS_FILE")
            }
            (None, None) => None,
        };

        match (issuer, jwks_source) {
            (None, None) => Ok(None),
            (Some(issuer), Some(jwks_source)) => {
                let audience =
                    token("RAVN_INGEST_AUDIENCE").unwrap_or_else(|| "ravn".to_string());
                Ok(Some(IngestAuthConfig { issuer, audience, jwks_source }))
            }
            _ => anyhow::bail!(
                "incomplete ingest-auth config: set RAVN_INGEST_OIDC_ISSUER and one of RAVN_INGEST_OIDC_JWKS_URL / RAVN_INGEST_OIDC_JWKS_FILE (or none)"
            ),
        }
    }

    /// Assemble enrollment config from the environment. Enrollment is enabled
    /// only when the bootstrap token *and* both CA file paths are set; if some
    /// but not all are present, that's a misconfiguration and we error.
    fn enroll_from_env(token: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<EnrollConfig>> {
        // The token may be given inline or, preferably, via a file (a systemd
        // credential) so it never lands in the Nix store or process env.
        let bootstrap = match token("RAVN_ENROLL_TOKEN") {
            Some(t) => Some(t),
            None => match token("RAVN_ENROLL_TOKEN_FILE") {
                Some(path) => Some(
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("reading RAVN_ENROLL_TOKEN_FILE at {path}"))?
                        .trim()
                        .to_string(),
                ),
                None => None,
            },
        };
        let ca_cert_path = token("RAVN_CA_CERT");
        let ca_key_path = token("RAVN_CA_KEY");

        match (bootstrap, ca_cert_path, ca_key_path) {
            (None, None, None) => Ok(None),
            (Some(bootstrap_token), Some(cert_path), Some(key_path)) => {
                let ca_cert_pem = std::fs::read_to_string(&cert_path)
                    .with_context(|| format!("reading RAVN_CA_CERT at {cert_path}"))?;
                let ca_key_pem = std::fs::read_to_string(&key_path)
                    .with_context(|| format!("reading RAVN_CA_KEY at {key_path}"))?;
                let cert_ttl_days = std::env::var("RAVN_CERT_TTL_DAYS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(90);
                Ok(Some(EnrollConfig { bootstrap_token, ca_cert_pem, ca_key_pem, cert_ttl_days }))
            }
            _ => anyhow::bail!(
                "incomplete enrollment config: set all of RAVN_ENROLL_TOKEN, RAVN_CA_CERT, RAVN_CA_KEY (or none)"
            ),
        }
    }
}
