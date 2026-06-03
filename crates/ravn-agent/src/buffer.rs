//! Local SQLite offline buffer (#21).
//!
//! When the control plane is unreachable, messages are enqueued here and
//! flushed on reconnect. Deduped by event id; bounded by a max row count
//! (oldest pruned). SQLite is bundled, so there is no system dependency.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use ravn_core::Message;
use rusqlite::{params, Connection};

pub struct Buffer {
    conn: Mutex<Connection>,
    max: usize,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Buffer {
    /// Open (or create) the buffer at `path`, capped at `max` rows.
    pub fn open(path: &str, max: usize) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening buffer at {path}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS outbox (
                 event_id    TEXT PRIMARY KEY,
                 message     TEXT NOT NULL,
                 enqueued_at INTEGER NOT NULL
             );",
        )
        .context("initializing buffer schema")?;
        Ok(Self { conn: Mutex::new(conn), max: max.max(1) })
    }

    /// Store a message, deduped by event id. Prunes oldest beyond the cap.
    pub fn enqueue(&self, msg: &Message) -> anyhow::Result<()> {
        let json = serde_json::to_string(msg).context("serializing message")?;
        let id = msg.event.id.to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO outbox (event_id, message, enqueued_at) VALUES (?1, ?2, ?3)",
            params![id, json, now_secs()],
        )?;
        // Drop oldest rows beyond the cap (max(0, …) avoids SQLite's
        // "negative LIMIT = unlimited" footgun).
        conn.execute(
            "DELETE FROM outbox WHERE event_id IN (
                 SELECT event_id FROM outbox ORDER BY enqueued_at ASC, rowid ASC
                 LIMIT max(0, (SELECT count(*) FROM outbox) - ?1)
             )",
            params![self.max as i64],
        )?;
        Ok(())
    }

    /// Oldest-first batch of buffered messages (with their ids). Unparseable
    /// rows are dropped.
    pub fn drain_batch(&self, n: usize) -> anyhow::Result<Vec<(String, Message)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT event_id, message FROM outbox ORDER BY enqueued_at ASC, rowid ASC LIMIT ?1")?;
        let rows = stmt
            .query_map(params![n as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = Vec::new();
        for (id, json) in rows {
            match serde_json::from_str::<Message>(&json) {
                Ok(m) => out.push((id, m)),
                Err(_) => {
                    conn.execute("DELETE FROM outbox WHERE event_id = ?1", params![id])?;
                }
            }
        }
        Ok(out)
    }

    pub fn delete(&self, event_id: &str) -> anyhow::Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM outbox WHERE event_id = ?1", params![event_id])?;
        Ok(())
    }

    pub fn len(&self) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ravn_core::{AgentId, Event, JournaldPayload, Payload, Severity};
    use uuid::Uuid;

    fn msg() -> Message {
        let now = Utc::now();
        Message::new(Event {
            id: Uuid::now_v7(),
            occurred_at: now,
            observed_at: now,
            agent_id: AgentId(Uuid::now_v7()),
            host: "h".into(),
            severity: Severity::Warning,
            title: "t".into(),
            category_hints: vec![],
            payload: Payload::Journald(JournaldPayload { message: "m".into(), ..Default::default() }),
        })
    }

    fn buf(max: usize) -> Buffer {
        Buffer::open(":memory:", max).unwrap()
    }

    #[test]
    fn enqueue_and_drain_roundtrips() {
        let b = buf(100);
        let m = msg();
        b.enqueue(&m).unwrap();
        assert_eq!(b.len().unwrap(), 1);
        let batch = b.drain_batch(10).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].1, m);
        b.delete(&batch[0].0).unwrap();
        assert_eq!(b.len().unwrap(), 0);
    }

    #[test]
    fn dedupes_by_event_id() {
        let b = buf(100);
        let m = msg();
        b.enqueue(&m).unwrap();
        b.enqueue(&m).unwrap();
        assert_eq!(b.len().unwrap(), 1);
    }

    #[test]
    fn prunes_beyond_cap() {
        let b = buf(3);
        for _ in 0..5 {
            b.enqueue(&msg()).unwrap();
        }
        assert_eq!(b.len().unwrap(), 3);
    }
}
