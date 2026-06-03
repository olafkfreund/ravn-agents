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

        Ok(Self { bind, log, database_url, nats_url, admin_token, viewer_token })
    }
}
