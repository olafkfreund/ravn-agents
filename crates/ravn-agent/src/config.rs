//! Agent configuration from an optional `--config <file>` plus environment.
//!
//! The NixOS module (#35) launches `ravnd --config <toml>`, so we accept that
//! file and read the fields the daemon needs today; environment variables
//! override it. A stable agent identity is established at enrollment (#19);
//! until then it comes from `RAVN_AGENT_ID` or is generated per start.

use std::path::PathBuf;

use anyhow::Context;
use ravn_core::Severity;
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
    /// Paths watched for config drift (#11); empty disables the watcher.
    pub config_drift_paths: Vec<PathBuf>,
    /// Whether the failed-unit D-Bus tap (#10) is enabled.
    pub failed_units_enable: bool,
    /// Watch the per-user systemd manager instead of the system one.
    pub systemd_user_bus: bool,
    /// Whether the update/NixOS-generation tap (#13) is enabled.
    pub updates_enable: bool,
    /// NixOS system profile symlink watched for generation changes.
    pub nix_profile: PathBuf,
    /// How often (seconds) the update tap polls the profile.
    pub update_poll_secs: u64,
    /// Whether to call local inference to explain events (#16).
    pub inference_enable: bool,
    /// OpenAI-compatible inference endpoint (llama-server).
    pub inference_endpoint: String,
    /// Model name sent to the inference endpoint.
    pub inference_model: String,
    /// Per-request inference timeout (seconds).
    pub inference_timeout_secs: u64,
    /// Batch events into a periodic digest instead of per-event explanations
    /// (#17). Requires `inference_enable`; when on, per-event enrichment is
    /// skipped to bound CPU.
    pub digest_enable: bool,
    /// How often (seconds) to emit a digest.
    pub digest_interval_secs: u64,
    /// Cap on events summarized per digest, to bound the prompt.
    pub digest_max_events: usize,
    /// Minimum severity an event needs to enter the digest (scope).
    pub digest_min_severity: Severity,
    /// How often (seconds) to publish a heartbeat (#20).
    pub heartbeat_interval_secs: u64,
    /// Path to the local SQLite offline buffer (#21).
    pub buffer_path: String,
    /// Max buffered messages before the oldest are pruned.
    pub buffer_max: usize,
    /// Control-plane enrollment endpoint base URL (#19); `None` skips enrollment.
    pub enroll_endpoint: Option<String>,
    /// Bootstrap token for first-time enrollment, read from a credential file.
    pub bootstrap_token: Option<String>,
    /// Directory holding the agent's mTLS credentials (key/cert/ca).
    pub cred_dir: PathBuf,
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
    #[serde(default)]
    inference: FileInference,
    #[serde(default)]
    enrollment: FileEnrollment,
}

#[derive(Debug, Default, Deserialize)]
struct FileEnrollment {
    endpoint: Option<String>,
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
    #[serde(default)]
    config_drift: FileConfigDrift,
    #[serde(default)]
    failed_units: FileFailedUnits,
    #[serde(default)]
    updates: FileUpdates,
}

#[derive(Debug, Default, Deserialize)]
struct FileInference {
    enable: Option<bool>,
    endpoint: Option<String>,
    model: Option<String>,
    #[serde(default)]
    digest: FileDigest,
}

#[derive(Debug, Default, Deserialize)]
struct FileDigest {
    enable: Option<bool>,
    interval_secs: Option<u64>,
    max_events: Option<usize>,
    min_severity: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfigDrift {
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileFailedUnits {
    enable: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct FileUpdates {
    enable: Option<bool>,
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

        // Env (comma/colon separated) takes precedence over the config file.
        let config_drift_paths: Vec<PathBuf> = match env_var("RAVN_CONFIG_DRIFT_PATHS") {
            Some(raw) => raw
                .split([',', ':'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect(),
            None => file.detection.config_drift.paths.into_iter().map(PathBuf::from).collect(),
        };

        let failed_units_enable = env_bool("RAVN_FAILED_UNITS")
            .or(file.detection.failed_units.enable)
            .unwrap_or(true);

        let systemd_user_bus = env_bool("RAVN_SYSTEMD_USER_BUS").unwrap_or(false);

        let updates_enable = env_bool("RAVN_UPDATES")
            .or(file.detection.updates.enable)
            .unwrap_or(true);

        let nix_profile = env_var("RAVN_NIX_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/nix/var/nix/profiles/system"));

        let update_poll_secs = env_var("RAVN_UPDATE_POLL_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(30)
            .max(1);

        let inference_enable = env_bool("RAVN_INFERENCE")
            .or(file.inference.enable)
            .unwrap_or(false);

        let inference_endpoint = env_var("RAVN_INFERENCE_ENDPOINT")
            .or(file.inference.endpoint)
            .unwrap_or_else(|| "http://127.0.0.1:18181".to_string());

        let inference_model = env_var("RAVN_INFERENCE_MODEL")
            .or(file.inference.model)
            .unwrap_or_else(|| "qwen3-1.7b-q4_k_m".to_string());

        let inference_timeout_secs = env_var("RAVN_INFERENCE_TIMEOUT_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(15)
            .max(1);

        let digest_enable = env_bool("RAVN_DIGEST")
            .or(file.inference.digest.enable)
            .unwrap_or(false);

        let digest_interval_secs = env_var("RAVN_DIGEST_INTERVAL_SECS")
            .and_then(|v| v.parse().ok())
            .or(file.inference.digest.interval_secs)
            .unwrap_or(3600)
            .max(1);

        let digest_max_events = env_var("RAVN_DIGEST_MAX_EVENTS")
            .and_then(|v| v.parse().ok())
            .or(file.inference.digest.max_events)
            .unwrap_or(100)
            .max(1);

        let digest_min_severity = env_var("RAVN_DIGEST_MIN_SEVERITY")
            .or(file.inference.digest.min_severity)
            .and_then(|s| severity_from_str(&s))
            .unwrap_or(Severity::Info);

        Ok(Self {
            server_url,
            agent_id,
            host,
            log,
            journald_enable,
            journald_min_priority,
            auth_enable,
            config_drift_paths,
            failed_units_enable,
            systemd_user_bus,
            updates_enable,
            nix_profile,
            update_poll_secs,
            inference_enable,
            inference_endpoint,
            inference_model,
            inference_timeout_secs,
            digest_enable,
            digest_interval_secs,
            digest_max_events,
            digest_min_severity,
            heartbeat_interval_secs: env_var("RAVN_HEARTBEAT_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(15)
                .max(1),
            buffer_path: env_var("RAVN_BUFFER_PATH").unwrap_or_else(|| {
                // Prefer systemd's StateDirectory when present.
                match env_var("STATE_DIRECTORY") {
                    Some(dir) => format!("{dir}/outbox.db"),
                    None => "ravn-outbox.db".to_string(),
                }
            }),
            buffer_max: env_var("RAVN_BUFFER_MAX")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10_000),
            enroll_endpoint: env_var("RAVN_ENROLL_ENDPOINT").or(file.enrollment.endpoint),
            bootstrap_token: bootstrap_token(),
            cred_dir: env_var("RAVN_CRED_DIR")
                .or_else(|| env_var("STATE_DIRECTORY").map(|d| format!("{d}/credentials")))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("ravn-credentials")),
        })
    }
}

/// Resolve the bootstrap token: `RAVN_BOOTSTRAP_TOKEN`, else the systemd
/// credential file `$CREDENTIALS_DIRECTORY/bootstrap-token` (#19). The token is
/// never placed in the world-readable Nix store.
fn bootstrap_token() -> Option<String> {
    if let Some(tok) = env_var("RAVN_BOOTSTRAP_TOKEN") {
        return Some(tok);
    }
    let dir = env_var("CREDENTIALS_DIRECTORY")?;
    let path = PathBuf::from(dir).join("bootstrap-token");
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
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

/// Parse a severity name (matching the wire `snake_case`).
fn severity_from_str(s: &str) -> Option<Severity> {
    match s.trim().to_ascii_lowercase().as_str() {
        "info" => Some(Severity::Info),
        "notice" => Some(Severity::Notice),
        "warning" => Some(Severity::Warning),
        "error" => Some(Severity::Error),
        "critical" => Some(Severity::Critical),
        _ => None,
    }
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
