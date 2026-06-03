//! journald reader tap (#9).
//!
//! Follows the systemd journal via `journalctl --follow --output=json` and emits
//! a normalized [`Event`] for each entry at or above a configured priority. This
//! is the foundation tap; auth/SSH (#12) and failed-unit context (#10) build on
//! the same journal stream.
//!
//! We shell out to `journalctl -o json` rather than linking libsystemd: it keeps
//! the build free of FFI, works across distros, and the mapping stays a pure,
//! testable function. Direct sd-journal is a future optimization.

use std::process::Stdio;

use anyhow::Context;
use chrono::{TimeZone, Utc};
use ravn_core::{AgentId, Event, Extra, JournaldPayload, Message, Payload, Severity};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

/// Follows the journal and emits events to a channel.
pub struct JournaldTap {
    pub agent_id: Uuid,
    pub host: String,
    /// Include generic entries whose syslog PRIORITY is <= this (0=emerg … 7=debug).
    pub min_priority: u8,
    /// Also classify auth/SSH/audit events (#12), regardless of priority.
    pub auth_enable: bool,
}

impl JournaldTap {
    /// Stream journal entries until the channel closes or journalctl exits.
    pub async fn run(&self, tx: Sender<Message>) -> anyhow::Result<()> {
        // Auth events are often info/notice, so when auth detection is on we
        // stream down to info and filter generic entries in-process.
        let stream_priority = if self.auth_enable {
            self.min_priority.max(6)
        } else {
            self.min_priority
        };

        let mut child = Command::new("journalctl")
            .args([
                "--follow",
                "--lines=0", // no backlog — only entries from now on
                "--output=json",
                &format!("--priority={stream_priority}"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning journalctl (is systemd-journald available?)")?;

        let stdout = child.stdout.take().context("capturing journalctl stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        tracing::info!(min_priority = self.min_priority, "journald tap following");

        while let Some(line) = lines.next_line().await? {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Map<String, Value>>(&line) {
                Ok(record) => {
                    if let Some(event) = self.classify(&record) {
                        if tx.send(Message::new(event)).await.is_err() {
                            break; // receiver gone — shutting down
                        }
                    }
                }
                Err(error) => tracing::debug!(%error, "skipping unparseable journal line"),
            }
        }

        let _ = child.kill().await;
        Ok(())
    }

    /// Classify a record: auth/SSH/audit first (#12), else the generic mapping.
    fn classify(&self, rec: &Map<String, Value>) -> Option<Event> {
        if self.auth_enable {
            if let Some(event) = super::auth::classify(rec, self.agent_id, &self.host) {
                return Some(event);
            }
        }
        self.record_to_event(rec)
    }

    /// Map a journalctl JSON record to an [`Event`]. Pure; unit-tested.
    pub fn record_to_event(&self, rec: &Map<String, Value>) -> Option<Event> {
        let field = |k: &str| rec.get(k).and_then(Value::as_str);

        let priority: u8 = field("PRIORITY").and_then(|p| p.parse().ok()).unwrap_or(6);
        if priority > self.min_priority {
            return None;
        }

        let message = field("MESSAGE")?.to_string();
        let unit = field("_SYSTEMD_UNIT").map(str::to_string);

        let occurred_at = field("__REALTIME_TIMESTAMP")
            .and_then(|t| t.parse::<i64>().ok())
            .and_then(|micros| Utc.timestamp_micros(micros).single())
            .unwrap_or_else(Utc::now);

        let mut extra = Extra::new();
        if let Some(id) = field("SYSLOG_IDENTIFIER") {
            extra.insert("syslog_identifier".to_string(), Value::from(id));
        }
        if let Some(pid) = field("_PID") {
            extra.insert("pid".to_string(), Value::from(pid));
        }

        let title = match &unit {
            Some(u) => format!("{u}: {}", truncate(&message, 80)),
            None => truncate(&message, 100),
        };

        Some(Event {
            id: Uuid::now_v7(),
            occurred_at,
            observed_at: Utc::now(),
            agent_id: AgentId(self.agent_id),
            host: self.host.clone(),
            severity: severity_from_priority(priority),
            title,
            category_hints: Vec::new(),
            payload: Payload::Journald(JournaldPayload {
                unit,
                priority: Some(priority),
                message,
                extra,
            }),
        })
    }
}

/// Map a syslog priority to a Ravn severity.
fn severity_from_priority(priority: u8) -> Severity {
    match priority {
        0..=2 => Severity::Critical, // emerg, alert, crit
        3 => Severity::Error,
        4 => Severity::Warning,
        5 => Severity::Notice,
        _ => Severity::Info, // info, debug
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::Source;

    fn tap() -> JournaldTap {
        JournaldTap { agent_id: Uuid::now_v7(), host: "test".into(), min_priority: 4, auth_enable: false }
    }

    fn record(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), Value::from(*v))).collect()
    }

    #[test]
    fn severity_ramps_with_priority() {
        assert_eq!(severity_from_priority(0), Severity::Critical);
        assert_eq!(severity_from_priority(2), Severity::Critical);
        assert_eq!(severity_from_priority(3), Severity::Error);
        assert_eq!(severity_from_priority(4), Severity::Warning);
        assert_eq!(severity_from_priority(5), Severity::Notice);
        assert_eq!(severity_from_priority(6), Severity::Info);
    }

    #[test]
    fn maps_a_unit_error_entry() {
        let rec = record(&[
            ("PRIORITY", "3"),
            ("MESSAGE", "connection refused"),
            ("_SYSTEMD_UNIT", "nginx.service"),
            ("SYSLOG_IDENTIFIER", "nginx"),
            ("_PID", "4242"),
            ("__REALTIME_TIMESTAMP", "1717405200000000"),
        ]);
        let ev = tap().record_to_event(&rec).expect("should map");
        assert_eq!(ev.severity, Severity::Error);
        assert_eq!(ev.source(), Source::Journald);
        assert!(ev.title.contains("nginx.service"));
        match ev.payload {
            Payload::Journald(p) => {
                assert_eq!(p.unit.as_deref(), Some("nginx.service"));
                assert_eq!(p.priority, Some(3));
                assert_eq!(p.message, "connection refused");
                assert_eq!(p.extra.get("pid").and_then(Value::as_str), Some("4242"));
            }
            other => panic!("expected journald payload, got {other:?}"),
        }
    }

    #[test]
    fn drops_entries_below_threshold() {
        let rec = record(&[("PRIORITY", "6"), ("MESSAGE", "just chatter")]);
        assert!(tap().record_to_event(&rec).is_none());
    }

    #[test]
    fn requires_a_message() {
        let rec = record(&[("PRIORITY", "3")]);
        assert!(tap().record_to_event(&rec).is_none());
    }
}
