//! Update / NixOS-generation detection (#13).
//!
//! On NixOS, a system update means a new system-profile generation: the
//! `/nix/var/nix/profiles/system` symlink re-points to `system-<N>-link`. We
//! poll that symlink and, when the generation changes, emit an event with the
//! from/to generation and a `nix store diff-closures` of what changed.
//!
//! Package-manager mechanisms (apt/dnf) are a future extension; on a host with
//! no NixOS system profile the tap stays idle.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use ravn_core::{AgentId, Event, Message, Payload, Severity, UpdatePayload};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

/// Polls the NixOS system profile for generation changes.
pub struct UpdateTap {
    pub agent_id: Uuid,
    pub host: String,
    /// The system profile symlink (default `/nix/var/nix/profiles/system`).
    pub profile: PathBuf,
    pub poll_interval: Duration,
}

impl UpdateTap {
    pub async fn run(&self, tx: Sender<Message>) -> anyhow::Result<()> {
        let Some(mut current) = read_generation(&self.profile) else {
            tracing::info!(
                profile = %self.profile.display(),
                "no NixOS system profile; update tap idle"
            );
            return Ok(());
        };

        let dir = self.profile.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut ticker = tokio::time::interval(self.poll_interval);
        tracing::info!(generation = current, "update tap watching system profile");

        loop {
            ticker.tick().await;
            if tx.is_closed() {
                return Ok(());
            }

            let Some(latest) = read_generation(&self.profile) else { continue };
            if latest == current {
                continue;
            }

            let changes = closure_diff(&dir, current, latest).await;
            let event = self.build_event(current, latest, changes);
            if tx.send(Message::new(event)).await.is_err() {
                return Ok(());
            }
            current = latest;
        }
    }

    fn build_event(&self, from: u64, to: u64, changes: Vec<String>) -> Event {
        let now = Utc::now();
        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(self.agent_id),
            host: self.host.clone(),
            severity: Severity::Notice,
            title: format!("system updated: generation {from} → {to}"),
            category_hints: Vec::new(),
            payload: Payload::Update(UpdatePayload {
                mechanism: "nixos".to_string(),
                from: Some(from.to_string()),
                to: Some(to.to_string()),
                changes,
                extra: Default::default(),
            }),
        }
    }
}

/// Current generation number from the profile symlink, if it points at one.
fn read_generation(profile: &Path) -> Option<u64> {
    let target = std::fs::read_link(profile).ok()?;
    let name = target.file_name()?.to_str()?;
    parse_generation(name)
}

/// `system-142-link` -> `142`. Works for any `<name>-<N>-link`.
fn parse_generation(target: &str) -> Option<u64> {
    target.strip_suffix("-link")?.rsplit('-').next()?.parse().ok()
}

/// Best-effort `nix store diff-closures` between two generations.
async fn closure_diff(dir: &Path, from: u64, to: u64) -> Vec<String> {
    let a = dir.join(format!("system-{from}-link"));
    let b = dir.join(format!("system-{to}-link"));
    let output = Command::new("nix")
        .args(["--extra-experimental-features", "nix-command", "store", "diff-closures"])
        .arg(&a)
        .arg(&b)
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(50)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_generation() {
        assert_eq!(parse_generation("system-142-link"), Some(142));
        assert_eq!(parse_generation("system-1-link"), Some(1));
    }

    #[test]
    fn parses_arbitrary_profile_name() {
        assert_eq!(parse_generation("nixos-system-host-3-link"), Some(3));
    }

    #[test]
    fn rejects_non_generation_targets() {
        assert_eq!(parse_generation("not-a-generation"), None);
        assert_eq!(parse_generation("system-x-link"), None);
        assert_eq!(parse_generation("garbage"), None);
    }
}
