//! Failed-unit detection via D-Bus (#10).
//!
//! Watches systemd unit state over D-Bus (`zbus`) and emits an event when a
//! unit enters `failed`. We poll `ListUnits` on a short interval and diff
//! against the previously-failed set — robust and far simpler than subscribing
//! to per-unit PropertiesChanged signals. For each new failure we attach the
//! systemd `Result` (e.g. `exit-code`, `oom-kill`) and recent journal lines.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

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
    /// A unit must stay `failed` continuously for at least this long before an
    /// event is emitted. Suppresses brief blips that systemd's own `Restart=`
    /// (or a planned restart) recovers before it matters. Zero = emit on the
    /// first observation (the original behaviour).
    pub grace: Duration,
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
        let mut tracker = GraceTracker::new();
        let mut ticker = tokio::time::interval(self.poll_interval);
        tracing::info!(
            user_bus = self.user_bus,
            grace_secs = self.grace.as_secs(),
            "failed-unit tap polling"
        );

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

            let current_failed: HashSet<String> = units
                .iter()
                .filter(|u| u.active_state == "failed")
                .map(|u| u.name.clone())
                .collect();

            // Only units that stayed `failed` for the whole grace window are
            // emitted (exactly once); a unit that recovered in time is dropped.
            let to_emit = tracker.observe(&current_failed, Instant::now(), self.grace);
            if to_emit.is_empty() {
                continue;
            }
            let by_name: HashMap<&str, &UnitStatus> =
                units.iter().map(|u| (u.name.as_str(), u)).collect();
            for name in to_emit {
                let Some(unit) = by_name.get(name.as_str()) else {
                    continue; // vanished between observation and emit; skip
                };
                let result = unit_result(&conn, &unit.unit_path)
                    .await
                    .unwrap_or_else(|| unit.sub_state.clone());
                let recent_log = recent_log(&unit.name, self.user_bus).await;
                let event = self.build_event(&unit.name, result, recent_log, &unit.active_state);
                if tx.send(Message::new(event)).await.is_err() {
                    return Ok(());
                }
            }
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

/// Tracks how long each unit has been continuously `failed` so the tap emits
/// only once a failure has persisted for the grace window — and exactly once per
/// failure episode. Pure: the caller injects `now`, so it is fully unit-testable.
#[derive(Default)]
struct GraceTracker {
    /// Failed units still inside their grace window: name -> first time seen failed.
    pending: HashMap<String, Instant>,
    /// Units already emitted for, while they remain failed (dedup guard).
    emitted: HashSet<String>,
}

impl GraceTracker {
    fn new() -> Self {
        Self::default()
    }

    /// Reconcile against the set of currently-`failed` unit names and return the
    /// units that have now been continuously failed for at least `grace` and have
    /// not yet been emitted. A unit that recovers before `grace` elapses is
    /// dropped silently; if it fails again later it re-arms. `grace == 0` emits on
    /// first observation (the original behaviour).
    fn observe(
        &mut self,
        current_failed: &HashSet<String>,
        now: Instant,
        grace: Duration,
    ) -> Vec<String> {
        // Forget units that have recovered: clears both a pending grace timer and
        // a prior emission, so a future failure of the same unit fires again.
        self.pending.retain(|name, _| current_failed.contains(name));
        self.emitted.retain(|name| current_failed.contains(name));

        let mut to_emit = Vec::new();
        for name in current_failed {
            if self.emitted.contains(name) {
                continue; // already emitted; still failed
            }
            let first_seen = *self.pending.entry(name.clone()).or_insert(now);
            if now.saturating_duration_since(first_seen) >= grace {
                self.pending.remove(name);
                self.emitted.insert(name.clone());
                to_emit.push(name.clone());
            }
        }
        to_emit
    }
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

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn grace_zero_emits_on_first_observation() {
        let mut t = GraceTracker::new();
        let now = Instant::now();
        assert_eq!(
            t.observe(&set(&["nginx.service"]), now, Duration::ZERO),
            vec!["nginx.service".to_string()]
        );
    }

    #[test]
    fn still_failed_unit_is_not_re_emitted() {
        let mut t = GraceTracker::new();
        let t0 = Instant::now();
        assert_eq!(t.observe(&set(&["a.service"]), t0, Duration::ZERO).len(), 1);
        // Next poll, still failed -> no re-emit.
        assert!(t
            .observe(&set(&["a.service"]), t0 + Duration::from_secs(5), Duration::ZERO)
            .is_empty());
    }

    #[test]
    fn brief_failure_then_recovery_is_suppressed() {
        let mut t = GraceTracker::new();
        let grace = Duration::from_secs(15);
        let t0 = Instant::now();
        // Seen failed, but inside the grace window -> not yet emitted.
        assert!(t.observe(&set(&["flap.service"]), t0, grace).is_empty());
        // Recovered before the window elapsed -> dropped, never emitted.
        assert!(t.observe(&set(&[]), t0 + Duration::from_secs(5), grace).is_empty());
        // And nothing fires later either — the timer was cleared on recovery.
        assert!(t.observe(&set(&[]), t0 + Duration::from_secs(30), grace).is_empty());
    }

    #[test]
    fn persistent_failure_emits_once_after_grace() {
        let mut t = GraceTracker::new();
        let grace = Duration::from_secs(15);
        let t0 = Instant::now();
        assert!(t.observe(&set(&["down.service"]), t0, grace).is_empty()); // within window
        assert!(t
            .observe(&set(&["down.service"]), t0 + Duration::from_secs(10), grace)
            .is_empty()); // still within
        assert_eq!(
            t.observe(&set(&["down.service"]), t0 + Duration::from_secs(15), grace),
            vec!["down.service".to_string()]
        );
        // Not again while it stays failed.
        assert!(t
            .observe(&set(&["down.service"]), t0 + Duration::from_secs(20), grace)
            .is_empty());
    }

    #[test]
    fn recovery_then_refailure_re_arms_and_emits_again() {
        let mut t = GraceTracker::new();
        let grace = Duration::from_secs(10);
        let t0 = Instant::now();
        assert!(t.observe(&set(&["svc.service"]), t0, grace).is_empty());
        assert_eq!(
            t.observe(&set(&["svc.service"]), t0 + Duration::from_secs(10), grace).len(),
            1
        );
        // Recovers, then fails again -> a fresh grace window, emits again after it.
        assert!(t.observe(&set(&[]), t0 + Duration::from_secs(15), grace).is_empty());
        assert!(t
            .observe(&set(&["svc.service"]), t0 + Duration::from_secs(20), grace)
            .is_empty());
        assert_eq!(
            t.observe(&set(&["svc.service"]), t0 + Duration::from_secs(30), grace).len(),
            1
        );
    }
}
