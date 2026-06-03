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

        Ok(Self { bind, log, database_url, nats_url, admin_token, viewer_token, enroll })
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
