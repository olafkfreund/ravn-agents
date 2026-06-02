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
}

impl Config {
    /// Build configuration from environment variables, applying defaults.
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("RAVN_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let bind: SocketAddr = bind
            .parse()
            .with_context(|| format!("RAVN_BIND is not a valid socket address: {bind:?}"))?;

        let log = std::env::var("RAVN_LOG").unwrap_or_else(|_| "info".to_string());

        Ok(Self { bind, log })
    }
}
