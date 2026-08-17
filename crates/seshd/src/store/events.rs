//! The append-only event log.
//!
//! There is deliberately no update or delete operation here, and there must
//! never be one. The log is the only authoritative state in SESH; everything
//! else is derived from it.

use anyhow::Result;

use super::{now_ms, Store};
use crate::event::{Event, NewEvent};

impl Store {
    /// Append an event and return it with its assigned id and timestamp.
    pub fn append(&self, new: NewEvent) -> Result<Event> {
        let ts_ms = now_ms();
        let actors = serde_json::to_string(&new.actors)?;
        let payload = serde_json::to_string(&new.payload)?;

        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO events (ts_ms, kind, actors, subject, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![ts_ms, new.kind, actors, new.subject, payload],
        )?;
        let id = conn.last_insert_rowid();

        Ok(Event {
            id,
            ts_ms,
            kind: new.kind,
            actors: new.actors,
            subject: new.subject,
            payload: new.payload,
        })
    }

    /// Read events with an id greater than `after_id`, oldest first.
    /// Pass `limit = -1` for no limit.
    pub fn read_since(&self, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, ts_ms, kind, actors, subject, payload
             FROM events WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![after_id, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (id, ts_ms, kind, actors, subject, payload) = row?;
            out.push(Event {
                id,
                ts_ms,
                kind,
                actors: serde_json::from_str(&actors)?,
                subject,
                payload: serde_json::from_str(&payload)?,
            });
        }
        Ok(out)
    }

    /// The highest id in the log, or 0 if it is empty.
    pub fn last_id(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let id: i64 =
            conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_increasing_ids_and_a_timestamp() {
        let store = Store::open_in_memory().unwrap();
        let a = store.append(NewEvent::new("a")).unwrap();
        let b = store.append(NewEvent::new("b")).unwrap();

        assert!(b.id > a.id, "ids must increase: {} then {}", a.id, b.id);
        assert!(a.ts_ms > 0, "timestamp must be set");
    }

    #[test]
    fn append_preserves_every_field() {
        let store = Store::open_in_memory().unwrap();
        let written = store
            .append(
                NewEvent::new("match.result")
                    .actor("tate")
                    .actor("sam")
                    .subject("mario-kart")
                    .payload(serde_json::json!({ "track": "rainbow-road" })),
            )
            .unwrap();

        let read = store.read_since(0, -1).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], written);
        assert_eq!(read[0].actors, vec!["tate".to_string(), "sam".to_string()]);
        assert_eq!(read[0].payload["track"], "rainbow-road");
    }

    #[test]
    fn read_since_returns_only_later_events_in_order() {
        let store = Store::open_in_memory().unwrap();
        let first = store.append(NewEvent::new("a")).unwrap();
        store.append(NewEvent::new("b")).unwrap();
        store.append(NewEvent::new("c")).unwrap();

        let rest = store.read_since(first.id, -1).unwrap();
        let kinds: Vec<_> = rest.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["b", "c"]);
    }

    #[test]
    fn read_since_respects_the_limit() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..5 {
            store.append(NewEvent::new(format!("k{i}"))).unwrap();
        }
        assert_eq!(store.read_since(0, 2).unwrap().len(), 2);
    }

    #[test]
    fn events_survive_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        {
            let store = Store::open(&path).unwrap();
            store
                .append(NewEvent::new("presence.arrived").actor("sam"))
                .unwrap();
        }

        let reopened = Store::open(&path).unwrap();
        let events = reopened.read_since(0, -1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actors, vec!["sam".to_string()]);
    }

    #[test]
    fn last_id_is_zero_on_an_empty_log() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.last_id().unwrap(), 0);
        let e = store.append(NewEvent::new("a")).unwrap();
        assert_eq!(store.last_id().unwrap(), e.id);
    }
}
