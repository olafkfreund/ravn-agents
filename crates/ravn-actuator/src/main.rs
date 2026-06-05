//! `ravn-actuator` entrypoint (#113).
//!
//! Configuration is environment-driven (set by the systemd unit, #120):
//! - `RAVN_ACTUATOR_SOCKET` — socket path (default `/run/ravn/actuator.sock`)
//! - `RAVN_COMMAND_PUBKEY`  — base64 Ed25519 public key the control plane signs with (required)
//! - `RAVN_ALLOWED_UID`     — the only uid permitted to connect (ravnd's uid)

use std::path::Path;

use anyhow::Context;
use ravn_actuator::{serve, SystemctlExecutor};
use ravn_crypto::verifying_key_from_b64;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let socket =
        std::env::var("RAVN_ACTUATOR_SOCKET").unwrap_or_else(|_| "/run/ravn/actuator.sock".into());
    let key_b64 = std::env::var("RAVN_COMMAND_PUBKEY")
        .context("RAVN_COMMAND_PUBKEY (base64 Ed25519 public key) must be set")?;
    let key = verifying_key_from_b64(&key_b64)
        .map_err(|e| anyhow::anyhow!("invalid RAVN_COMMAND_PUBKEY: {e}"))?;
    let allowed_uid = std::env::var("RAVN_ALLOWED_UID").ok().and_then(|s| s.parse().ok());

    serve(Path::new(&socket), key, SystemctlExecutor, allowed_uid).await
}
