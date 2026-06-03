//! Agent configuration from an optional `--config <file>` plus environment.
//!
//! The NixOS module (#35) launches `ravnd --config <toml>`, so we accept that
//! file and read the fields the daemon needs today; environment variables
//! override it. A stable agent identity is established at enrollment (#19);
//! until then it comes from `RAVN_AGENT_ID` or is generated per start.

use anyhow::Context;
use serde::Deserialize;
use uuid::Uuid;

/// Resolved agent configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Control-plane transport URL (`nats://…` or `ws://…`).
    pub server_url: String,
    /// This agent's identity.
    pub agent_id: Uuid,
    /// Hostname reported on emitted events.
    pub host: String,
    /// Default tracing filter when `RUST_LOG` is unset.
    pub log: String,
    /// Whether the journald detection tap (#9) is enabled.
    pub journald_enable: bool,
    /// Minimum syslog priority the journald tap emits (0=emerg … 7=debug).
    pub journald_min_priority: u8,
    /// Whether the auth/SSH/audit classifier (#12) is enabled.
    pub auth_enable: bool,
}

/// Subset of the TOML config file the daemon currently reads.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    server: FileServer,
    #[serde(default)]
    log: FileLog,
    #[serde(default)]
    detection: FileDetection,
}

#[derive(Debug, Default, Deserialize)]
struct FileServer {
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileLog {
    level: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileDetection {
    #[serde(default)]
    journald: FileJournald,
    #[serde(default)]
    auth: FileAuth,
}

#[derive(Debug, Default, Deserialize)]
struct FileJournald {
    enable: Option<bool>,
    priority: Option<u8>,
}

#[derive(Debug, Default, Deserialize)]
struct FileAuth {
    enable: Option<bool>,
}

impl Config {
    /// Load configuration from CLI args and the environment.
    pub fn load() -> anyhow::Result<Self> {
        let file = match config_path_from_args(std::env::args()) {
            Some(path) => read_file_config(&path)?,
            None => FileConfig::default(),
        };

        let server_url = env_var("RAVN_SERVER_URL")
            .or(file.server.url)
            .unwrap_or_else(|| "nats://127.0.0.1:14222".to_string());

        let agent_id = match env_var("RAVN_AGENT_ID") {
            Some(raw) => raw.parse().context("RAVN_AGENT_ID is not a valid UUID")?,
            None => Uuid::now_v7(),
        };

        let host = env_var("RAVN_HOST")
            .or_else(|| env_var("HOSTNAME"))
            .unwrap_or_else(|| "unknown".to_string());

        let log = env_var("RAVN_LOG")
            .or(file.log.level)
            .unwrap_or_else(|| "info".to_string());

        let journald_enable = env_bool("RAVN_JOURNALD")
            .or(file.detection.journald.enable)
            .unwrap_or(true);

        let journald_min_priority = env_var("RAVN_JOURNALD_PRIORITY")
            .and_then(|v| v.parse().ok())
            .or(file.detection.journald.priority)
            .unwrap_or(4) // warning and above
            .min(7);

        let auth_enable = env_bool("RAVN_AUTH")
            .or(file.detection.auth.enable)
            .unwrap_or(true);

        Ok(Self {
            server_url,
            agent_id,
            host,
            log,
            journald_enable,
            journald_min_priority,
            auth_enable,
        })
    }
}

/// Read `--config <path>` (or `--config=<path>`) from an argument iterator.
fn config_path_from_args(args: impl Iterator<Item = String>) -> Option<String> {
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--config" {
            return args.next();
        }
        if let Some(rest) = arg.strip_prefix("--config=") {
            return Some(rest.to_string());
        }
    }
    None
}

fn read_file_config(path: &str) -> anyhow::Result<FileConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {path}"))?;
    toml::from_str(&contents).with_context(|| format!("parsing config file {path}"))
}

/// `std::env::var`, treating empty as unset.
fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Parse a boolean-ish env var (`1/true/yes/on` vs `0/false/no/off`).
fn env_bool(key: &str) -> Option<bool> {
    match env_var(key)?.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_flag_spaced() {
        let args = ["ravnd", "--config", "/etc/ravn.toml"].map(String::from);
        assert_eq!(config_path_from_args(args.into_iter()).as_deref(), Some("/etc/ravn.toml"));
    }

    #[test]
    fn parses_config_flag_equals() {
        let args = ["ravnd", "--config=/etc/ravn.toml"].map(String::from);
        assert_eq!(config_path_from_args(args.into_iter()).as_deref(), Some("/etc/ravn.toml"));
    }

    #[test]
    fn no_config_flag_is_none() {
        let args = ["ravnd"].map(String::from);
        assert_eq!(config_path_from_args(args.into_iter()), None);
    }

    #[test]
    fn file_config_reads_server_url() {
        let toml = r#"
            [server]
            url = "nats://example:4222"
            [log]
            level = "debug"
        "#;
        let cfg: FileConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.server.url.as_deref(), Some("nats://example:4222"));
        assert_eq!(cfg.log.level.as_deref(), Some("debug"));
    }
}
