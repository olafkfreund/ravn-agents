//! Auth / SSH / audit classifier (#12).
//!
//! Runs over the same journal stream as the journald tap (#9) and recognises
//! access events — SSH logins and failures, sudo, sessions, and auditd
//! USER_AUTH records — emitting typed [`AuthPayload`] events. Pure and
//! unit-tested; log-format variance is handled defensively.

use chrono::{TimeZone, Utc};
use ravn_core::{AgentId, AuthPayload, Event, Payload, Severity};
use serde_json::{Map, Value};
use uuid::Uuid;

struct Detected {
    action: &'static str,
    user: Option<String>,
    remote: Option<String>,
    succeeded: bool,
    severity: Severity,
}

/// Classify a journal record as an auth event, if it is one.
pub fn classify(rec: &Map<String, Value>, agent_id: Uuid, host: &str) -> Option<Event> {
    let field = |k: &str| rec.get(k).and_then(Value::as_str);
    let ident = field("SYSLOG_IDENTIFIER").or_else(|| field("_COMM")).unwrap_or("");
    let transport = field("_TRANSPORT").unwrap_or("");
    let msg = field("MESSAGE")?;

    let d = if ident == "sshd" {
        classify_ssh(msg)
    } else if ident == "sudo" {
        classify_sudo(msg)
    } else if transport == "audit" || ident == "audit" || ident == "auditd" {
        classify_audit(msg)
    } else if msg.starts_with("New session") || msg.contains("session opened for user") {
        Some(Detected {
            action: "session_open",
            user: word_after(msg, "user "),
            remote: None,
            succeeded: true,
            severity: Severity::Info,
        })
    } else {
        None
    }?;

    let occurred_at = field("__REALTIME_TIMESTAMP")
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(|us| Utc.timestamp_micros(us).single())
        .unwrap_or_else(Utc::now);

    let title = match (&d.user, &d.remote) {
        (Some(u), Some(r)) => format!("{}: {u} from {r}", d.action),
        (Some(u), None) => format!("{}: {u}", d.action),
        (None, Some(r)) => format!("{} from {r}", d.action),
        (None, None) => d.action.to_string(),
    };

    Some(Event {
        id: Uuid::now_v7(),
        occurred_at,
        observed_at: Utc::now(),
        agent_id: AgentId(agent_id),
        host: host.to_string(),
        severity: d.severity,
        title,
        category_hints: Vec::new(),
        payload: Payload::Auth(AuthPayload {
            action: d.action.to_string(),
            user: d.user,
            remote_addr: d.remote,
            succeeded: d.succeeded,
            extra: Default::default(),
        }),
    })
}

fn classify_ssh(msg: &str) -> Option<Detected> {
    if let Some(rest) = msg.strip_prefix("Accepted ") {
        return Some(Detected {
            action: "ssh_login",
            user: word_after(rest, "for "),
            remote: word_after(rest, "from "),
            succeeded: true,
            severity: Severity::Notice,
        });
    }
    if msg.starts_with("Failed password for") || msg.starts_with("Failed publickey for") {
        let user = if msg.contains("invalid user") {
            word_after(msg, "invalid user ")
        } else {
            word_after(msg, "for ")
        };
        return Some(Detected {
            action: "ssh_failed",
            user,
            remote: word_after(msg, "from "),
            succeeded: false,
            severity: Severity::Warning,
        });
    }
    if msg.starts_with("Invalid user") {
        return Some(Detected {
            action: "ssh_invalid_user",
            user: word_after(msg, "Invalid user "),
            remote: word_after(msg, "from "),
            succeeded: false,
            severity: Severity::Warning,
        });
    }
    None
}

fn classify_sudo(msg: &str) -> Option<Detected> {
    if msg.contains("authentication failure") || msg.contains("incorrect password") {
        return Some(Detected {
            action: "sudo_failed",
            user: sudo_user(msg),
            remote: None,
            succeeded: false,
            severity: Severity::Warning,
        });
    }
    if msg.contains("TTY=") && msg.contains("COMMAND=") {
        return Some(Detected {
            action: "sudo",
            user: sudo_user(msg),
            remote: None,
            succeeded: true,
            severity: Severity::Notice,
        });
    }
    None
}

fn classify_audit(msg: &str) -> Option<Detected> {
    if !(msg.contains("USER_AUTH") || msg.contains("USER_LOGIN")) {
        return None;
    }
    let succeeded = msg.contains("res=success");
    Some(Detected {
        action: "audit_user_auth",
        user: word_after(msg, "acct="),
        remote: word_after(msg, "addr=").filter(|a| a != "?"),
        succeeded,
        severity: if succeeded { Severity::Info } else { Severity::Warning },
    })
}

/// The invoking user of a sudo line (handles both the `user : TTY=` and the
/// pam `ruser=`/`logname=` forms).
fn sudo_user(msg: &str) -> Option<String> {
    if let Some(u) = word_after(msg, "ruser=").filter(|u| !u.is_empty()) {
        return Some(u);
    }
    if let Some(head) = msg.split(" : ").next() {
        let head = head.trim();
        if !head.is_empty() && !head.contains(' ') {
            return Some(head.to_string());
        }
    }
    word_after(msg, "logname=")
}

/// First whitespace-delimited token after `marker`, trimmed of trailing
/// punctuation.
fn word_after(s: &str, marker: &str) -> Option<String> {
    let start = s.find(marker)? + marker.len();
    s[start..]
        .split_whitespace()
        .next()
        .map(|w| w.trim_end_matches([':', ';', ',', '.']).to_string())
        .filter(|w| !w.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravn_core::Source;

    fn rec(ident: &str, msg: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("SYSLOG_IDENTIFIER".into(), Value::from(ident));
        m.insert("MESSAGE".into(), Value::from(msg));
        m
    }

    fn auth(ident: &str, msg: &str) -> Event {
        classify(&rec(ident, msg), Uuid::now_v7(), "h").expect("should classify")
    }

    fn payload(e: &Event) -> &AuthPayload {
        match &e.payload {
            Payload::Auth(p) => p,
            other => panic!("expected auth payload, got {other:?}"),
        }
    }

    #[test]
    fn ssh_accepted_login() {
        let e = auth("sshd", "Accepted publickey for alice from 10.0.0.5 port 53122 ssh2");
        assert_eq!(e.source(), Source::Auth);
        assert_eq!(e.severity, Severity::Notice);
        let p = payload(&e);
        assert_eq!(p.action, "ssh_login");
        assert_eq!(p.user.as_deref(), Some("alice"));
        assert_eq!(p.remote_addr.as_deref(), Some("10.0.0.5"));
        assert!(p.succeeded);
    }

    #[test]
    fn ssh_failed_password() {
        let e = auth("sshd", "Failed password for bob from 203.0.113.9 port 40000 ssh2");
        let p = payload(&e);
        assert_eq!(p.action, "ssh_failed");
        assert_eq!(p.user.as_deref(), Some("bob"));
        assert_eq!(p.remote_addr.as_deref(), Some("203.0.113.9"));
        assert!(!p.succeeded);
    }

    #[test]
    fn ssh_failed_invalid_user() {
        let e = auth("sshd", "Failed password for invalid user root from 1.2.3.4 port 22 ssh2");
        let p = payload(&e);
        assert_eq!(p.user.as_deref(), Some("root"));
        assert_eq!(p.remote_addr.as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn sudo_command() {
        let e = auth("sudo", "olaf : TTY=pts/0 ; PWD=/home ; USER=root ; COMMAND=/bin/systemctl restart nginx");
        let p = payload(&e);
        assert_eq!(p.action, "sudo");
        assert_eq!(p.user.as_deref(), Some("olaf"));
        assert!(p.succeeded);
    }

    #[test]
    fn sudo_failure() {
        let e = auth("sudo", "pam_unix(sudo:auth): authentication failure; logname=olaf uid=1000 ruser=olaf rhost= user=root");
        assert_eq!(e.severity, Severity::Warning);
        let p = payload(&e);
        assert_eq!(p.action, "sudo_failed");
        assert_eq!(p.user.as_deref(), Some("olaf"));
        assert!(!p.succeeded);
    }

    #[test]
    fn non_auth_returns_none() {
        assert!(classify(&rec("kernel", "usb 1-1: new high-speed USB device"), Uuid::now_v7(), "h").is_none());
    }
}
