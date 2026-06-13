//! `ravn-actuator` entrypoint (#113).
//!
//! Configuration is environment-driven (set by the systemd unit, #120):
//! - `RAVN_ACTUATOR_SOCKET` — socket path (default `/run/ravn/actuator.sock`)
//! - `RAVN_COMMAND_KEYRING` — path to the pinned command-signing keyring JSON
//!   (#150). When set, the actuator trusts the whole keyring and reloads it on
//!   each command, so it honours rotation overlaps. Usually `ravnd`'s
//!   `command_keyring.json` in its credential dir.
//! - `RAVN_COMMAND_PUBKEY`  — base64 Ed25519 public key the control plane signs
//!   with. Used as the initial/fallback trust when `RAVN_COMMAND_KEYRING` is
//!   unset or not yet written. At least one of the two must be provided.
//! - `RAVN_ALLOWED_UID`     — the only uid permitted to connect (ravnd's uid)

use std::path::{Path, PathBuf};

use anyhow::Context;
use ravn_actuator::{serve, SystemctlExecutor};
use ravn_crypto::{verifying_key_from_b64, Keyring};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let socket =
        std::env::var("RAVN_ACTUATOR_SOCKET").unwrap_or_else(|_| "/run/ravn/actuator.sock".into());
    let allowed_uid = std::env::var("RAVN_ALLOWED_UID").ok().and_then(|s| s.parse().ok());

    // Prefer the pinned keyring file (rotation-aware, #150); fall back to the
    // single advertised pubkey for backward compatibility.
    let keyring_path: Option<PathBuf> =
        std::env::var("RAVN_COMMAND_KEYRING").ok().filter(|s| !s.is_empty()).map(PathBuf::from);

    let initial = match keyring_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(json) => Keyring::from_json(&json)
            .map_err(|e| anyhow::anyhow!("invalid RAVN_COMMAND_KEYRING: {e}"))?,
        None => {
            let key_b64 = std::env::var("RAVN_COMMAND_PUBKEY").context(
                "set RAVN_COMMAND_KEYRING (preferred) or RAVN_COMMAND_PUBKEY (base64 Ed25519 public key)",
            )?;
            let key = verifying_key_from_b64(&key_b64)
                .map_err(|e| anyhow::anyhow!("invalid RAVN_COMMAND_PUBKEY: {e}"))?;
            Keyring::single(key)
        }
    };

    serve(Path::new(&socket), initial, keyring_path.as_deref(), SystemctlExecutor, allowed_uid).await
}
