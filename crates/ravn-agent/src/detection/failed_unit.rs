//! Failed-unit detection via D-Bus (#10).
//!
//! Watches systemd unit state over D-Bus (`zbus`) and emits an event when a
//! unit enters `failed`. We poll `ListUnits` on a short interval and diff
//! against the previously-failed set — robust and far simpler than subscribing
//! to per-unit PropertiesChanged signals. For each new failure we attach the
//! systemd `Result` (e.g. `exit-code`, `oom-kill`) and recent journal lines.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use ravn_core::{AgentId, Event, FailedUnitPayload, Message, Payload, Severity};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use uuid::Uuid;
use zbus::zvariant::{OwnedObjectPath, Type};
use zbus::{proxy, Connection, Proxy};

/// One row of `org.freedesktop.systemd1.Manager.ListUnits` (`a(ssssssouso)`).
/// All ten fields are required to match the D-Bus signature; several aren't
/// read but must be present for positional deserialization.
#[derive(Debug, Clone, Deserialize, Type)]
#[allow(dead_code)]
pub struct UnitStatus {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub followed: String,
    pub unit_path: OwnedObjectPath,
    pub job_id: u32,
    pub job_type: String,
    pub job_path: OwnedObjectPath,
}

#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Manager {
    fn list_units(&self) -> zbus::Result<Vec<UnitStatus>>;
}

/// Polls systemd over D-Bus for units entering the failed state.
pub struct FailedUnitTap {
    pub agent_id: Uuid,
    pub host: String,
    /// Use the per-user systemd manager (session bus) instead of the system one.
    pub user_bus: bool,
    pub poll_interval: Duration,
}

impl FailedUnitTap {
    pub async fn run(&self, tx: Sender<Message>) -> anyhow::Result<()> {
        let conn = if self.user_bus {
            Connection::session().await
        } else {
            Connection::system().await
        }
        .context("connecting to D-Bus")?;

        let manager = ManagerProxy::new(&conn).await.context("systemd manager proxy")?;
        let mut failed: HashSet<String> = HashSet::new();
        let mut ticker = tokio::time::interval(self.poll_interval);
        tracing::info!(user_bus = self.user_bus, "failed-unit tap polling");

        loop {
            ticker.tick().await;
            if tx.is_closed() {
                return Ok(());
            }

            let units = match manager.list_units().await {
                Ok(u) => u,
                Err(error) => {
                    tracing::warn!(%error, "ListUnits failed");
                    continue;
                }
            };

            for unit in newly_failed(&failed, &units) {
                let result = unit_result(&conn, &unit.unit_path)
                    .await
                    .unwrap_or_else(|| unit.sub_state.clone());
                let recent_log = recent_log(&unit.name, self.user_bus).await;
                let event = self.build_event(&unit.name, result, recent_log, &unit.active_state);
                if tx.send(Message::new(event)).await.is_err() {
                    return Ok(());
                }
            }

            failed = units
                .iter()
                .filter(|u| u.active_state == "failed")
                .map(|u| u.name.clone())
                .collect();
        }
    }

    fn build_event(&self, unit: &str, result: String, recent_log: Vec<String>, active_state: &str) -> Event {
        let now = Utc::now();
        let mut payload = FailedUnitPayload {
            unit: unit.to_string(),
            result,
            recent_log,
            extra: Default::default(),
        };
        payload
            .extra
            .insert("active_state".to_string(), active_state.into());

        Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(self.agent_id),
            host: self.host.clone(),
            severity: Severity::Error,
            title: format!("unit failed: {unit}"),
            category_hints: Vec::new(),
            payload: Payload::FailedUnit(payload),
        }
    }
}

/// Units that are failed now and weren't in the previous failed set. Pure.
fn newly_failed<'a>(prev: &HashSet<String>, units: &'a [UnitStatus]) -> Vec<&'a UnitStatus> {
    units
        .iter()
        .filter(|u| u.active_state == "failed" && !prev.contains(&u.name))
        .collect()
}

/// Best-effort fetch of a unit's systemd `Result` property.
async fn unit_result(conn: &Connection, path: &OwnedObjectPath) -> Option<String> {
    let proxy = Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        path.clone(),
        "org.freedesktop.systemd1.Service",
    )
    .await
    .ok()?;
    proxy.get_property::<String>("Result").await.ok()
}

/// Recent journal lines for a unit (context for the explanation).
async fn recent_log(unit: &str, user: bool) -> Vec<String> {
    let mut cmd = Command::new("journalctl");
    if user {
        cmd.arg("--user");
    }
    cmd.args(["-u", unit, "-n", "12", "-o", "cat", "--no-pager"]);
    match cmd.output().await {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, active: &str) -> UnitStatus {
        let path = OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/x").unwrap();
        UnitStatus {
            name: name.to_string(),
            description: String::new(),
            load_state: "loaded".into(),
            active_state: active.to_string(),
            sub_state: "failed".into(),
            followed: String::new(),
            unit_path: path.clone(),
            job_id: 0,
            job_type: String::new(),
            job_path: path,
        }
    }

    #[test]
    fn detects_new_failures_only() {
        let units = vec![unit("nginx.service", "failed"), unit("sshd.service", "active")];
        let prev = HashSet::new();
        let found = newly_failed(&prev, &units);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "nginx.service");
    }

    #[test]
    fn already_failed_units_are_not_re_emitted() {
        let units = vec![unit("nginx.service", "failed")];
        let prev: HashSet<String> = ["nginx.service".to_string()].into_iter().collect();
        assert!(newly_failed(&prev, &units).is_empty());
    }
}
