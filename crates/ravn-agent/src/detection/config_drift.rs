//! Config-drift watcher (#11).
//!
//! Watches a configured set of paths via inotify (the `notify` crate), hashes
//! their contents, and emits a diff-bearing event when a file changes. The
//! "what changed in config" signal. Core comparison logic is a pure function so
//! it can be unit-tested without touching the filesystem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use chrono::Utc;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use ravn_core::{AgentId, ConfigDriftPayload, Event as RavnEvent, Message, Payload, Severity};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

/// Cap on file size we keep as text for diffing (256 KiB).
const MAX_TEXT_BYTES: usize = 256 * 1024;

/// Watches paths and emits drift events.
pub struct ConfigDriftTap {
    pub agent_id: Uuid,
    pub host: String,
    pub paths: Vec<PathBuf>,
}

/// Remembered state of a watched file.
struct FileState {
    hash: String,
    text: Option<String>,
}

impl ConfigDriftTap {
    pub async fn run(&self, tx: Sender<Message>) -> anyhow::Result<()> {
        let paths = self.paths.clone();
        let agent_id = self.agent_id;
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || watch(paths, agent_id, host, tx))
            .await
            .map_err(|e| anyhow::anyhow!("config-drift task panicked: {e}"))?
    }
}

fn watch(paths: Vec<PathBuf>, agent_id: Uuid, host: String, tx: Sender<Message>) -> anyhow::Result<()> {
    let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = raw_tx.send(res);
    })?;

    let mut baseline: HashMap<PathBuf, FileState> = HashMap::new();
    for p in &paths {
        if let Err(error) = watcher.watch(p, RecursiveMode::Recursive) {
            tracing::warn!(path = %p.display(), %error, "cannot watch path");
            continue;
        }
        // Seed explicit files so the first change carries a diff.
        if p.is_file() {
            if let Ok(bytes) = std::fs::read(p) {
                if let Some((_, state)) = evaluate(&p.display().to_string(), None, &bytes) {
                    baseline.insert(p.clone(), state);
                }
            }
        }
    }
    tracing::info!(count = paths.len(), "config-drift watching");

    loop {
        match raw_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => {
                if !matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    continue;
                }
                for path in event.paths {
                    if path.is_dir() {
                        continue;
                    }
                    if let Some(ev) = process(&path, &mut baseline, agent_id, &host) {
                        if tx.blocking_send(Message::new(ev)).is_err() {
                            return Ok(()); // receiver gone
                        }
                    }
                }
            }
            Ok(Err(error)) => tracing::debug!(%error, "watch error"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if tx.is_closed() {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Read a changed path and build an event, updating the baseline.
fn process(
    path: &Path,
    baseline: &mut HashMap<PathBuf, FileState>,
    agent_id: Uuid,
    host: &str,
) -> Option<RavnEvent> {
    let key = path.display().to_string();
    match std::fs::read(path) {
        Ok(bytes) => {
            let (payload, state) = evaluate(&key, baseline.get(path), &bytes)?;
            baseline.insert(path.to_path_buf(), state);
            Some(build_event(payload, agent_id, host))
        }
        Err(_) => {
            // Disappeared/unreadable — report removal if we knew it.
            let prev = baseline.remove(path)?;
            let payload = ConfigDriftPayload {
                path: key,
                old_hash: Some(prev.hash),
                new_hash: "(removed)".to_string(),
                diff: None,
                extra: Default::default(),
            };
            Some(build_event(payload, agent_id, host))
        }
    }
}

/// Pure drift evaluation: returns the payload + new state, or `None` if the
/// content is unchanged. Filesystem-free for testability.
fn evaluate(
    path: &str,
    prev: Option<&FileState>,
    new_bytes: &[u8],
) -> Option<(ConfigDriftPayload, FileState)> {
    let new_hash = hex::encode(Sha256::digest(new_bytes));
    if prev.is_some_and(|p| p.hash == new_hash) {
        return None; // dedupe repeated/no-op events
    }

    let new_text = if new_bytes.len() <= MAX_TEXT_BYTES {
        String::from_utf8(new_bytes.to_vec()).ok()
    } else {
        None
    };

    let diff = match (prev.and_then(|p| p.text.as_deref()), new_text.as_deref()) {
        (Some(old), Some(new)) => Some(unified_diff(path, old, new)),
        _ => None,
    };

    let payload = ConfigDriftPayload {
        path: path.to_string(),
        old_hash: prev.map(|p| p.hash.clone()),
        new_hash: new_hash.clone(),
        diff,
        extra: Default::default(),
    };
    Some((payload, FileState { hash: new_hash, text: new_text }))
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(path, path)
        .to_string();
    // Bound the stored diff.
    if diff.len() > MAX_TEXT_BYTES {
        format!("{}\n… (diff truncated)", &diff[..MAX_TEXT_BYTES])
    } else {
        diff
    }
}

fn build_event(payload: ConfigDriftPayload, agent_id: Uuid, host: &str) -> RavnEvent {
    let now = Utc::now();
    RavnEvent {
        id: Uuid::now_v7(),
        occurred_at: now,
        observed_at: now,
        agent_id: AgentId(agent_id),
        host: host.to_string(),
        severity: Severity::Warning,
        title: format!("config drift: {}", payload.path),
        category_hints: Vec::new(),
        payload: Payload::ConfigDrift(payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_has_no_old_hash_or_diff() {
        let (p, state) = evaluate("/etc/x.conf", None, b"hello\n").expect("new file");
        assert!(p.old_hash.is_none());
        assert!(p.diff.is_none());
        assert_eq!(state.hash, p.new_hash);
        assert!(!p.new_hash.is_empty());
    }

    #[test]
    fn unchanged_content_is_dropped() {
        let (_, state) = evaluate("/etc/x.conf", None, b"same\n").unwrap();
        assert!(evaluate("/etc/x.conf", Some(&state), b"same\n").is_none());
    }

    #[test]
    fn changed_text_produces_a_diff() {
        let (_, old) = evaluate("/etc/sshd_config", None, b"PermitRootLogin yes\n").unwrap();
        let (p, _) = evaluate("/etc/sshd_config", Some(&old), b"PermitRootLogin no\n").expect("changed");
        assert!(p.old_hash.is_some());
        assert_ne!(p.old_hash.as_deref(), Some(p.new_hash.as_str()));
        let diff = p.diff.expect("diff present");
        assert!(diff.contains("-PermitRootLogin yes"));
        assert!(diff.contains("+PermitRootLogin no"));
    }
}
