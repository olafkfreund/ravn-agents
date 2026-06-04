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
    /// Shared inference endpoint for explaining K8s events (#58). `Some`
    /// enables async explanation generation.
    pub inference: Option<InferenceConfig>,
    /// Portal user authentication via OIDC + RBAC (#26). `Some` enables
    /// OIDC-bearer validation on the API (alongside the static dev tokens).
    pub oidc: Option<OidcConfig>,
}

/// Portal user OIDC + RBAC configuration (#26).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// OIDC issuer URL. `RAVN_OIDC_ISSUER` (enables user auth).
    pub issuer: String,
    /// Where to load the IdP JWKS from. `RAVN_OIDC_JWKS_URL` / `_FILE`.
    pub jwks_source: JwksSource,
    /// Expected token audience / OIDC client id. `RAVN_OIDC_AUDIENCE`.
    pub audience: Option<String>,
    /// Public client id the SPA uses for the auth-code+PKCE flow.
    /// `RAVN_OIDC_CLIENT_ID` (defaults to `audience`).
    pub client_id: Option<String>,
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
        let inference = Self::inference_from_env(&token)?;
        let oidc = Self::oidc_from_env(&token)?;

        Ok(Self {
            bind,
            log,
            database_url,
            nats_url,
            admin_token,
            viewer_token,
            enroll,
            ingest_auth,
            inference,
            oidc,
        })
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
        let audience = token("RAVN_OIDC_AUDIENCE");
        let client_id = token("RAVN_OIDC_CLIENT_ID").or_else(|| audience.clone());
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
