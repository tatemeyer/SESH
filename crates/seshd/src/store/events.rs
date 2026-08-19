//! The append-only event log.
//!
//! There is deliberately no update or delete operation here, and there must
//! never be one. The log is the only authoritative state in SESH; everything
//! else is derived from it.

use anyhow::Result;

use super::Store;
use crate::clock::CLOCK_SYNCED;
use crate::event::{Event, NewEvent};

impl Store {
    /// Append an event and return it with its assigned id and timestamp.
    pub fn append(&self, mut new: NewEvent) -> Result<Event> {
        let ts_ms = self.clock.now_ms();

        // A row stamped from a clock that has not been corrected yet says so,
        // in the payload, next to the claim it qualifies. Applied here because
        // this is already the only place that stamps time, so no caller can
        // forget it. Added, never overwritten: a caller that has answered the
        // question owns the answer.
        if !self.clock.synced() {
            if let Some(object) = new.payload.as_object_mut() {
                object
                    .entry(CLOCK_SYNCED)
                    .or_insert_with(|| serde_json::Value::Bool(false));
            }
        }

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
    use std::sync::Arc;

    use super::*;
    use crate::clock::{TestClock, CLOCK_SYNCED};

    /// A store whose clock the test drives. Returns both so the test can move
    /// the clock after the store is built.
    fn store_with_clock(wall_ms: i64) -> (Store, Arc<TestClock>) {
        let clock = Arc::new(TestClock::new(wall_ms));
        let store = Store::open_in_memory().unwrap().with_clock(clock.clone());
        (store, clock)
    }

    #[test]
    fn the_timestamp_comes_from_the_clock() {
        let (store, _clock) = store_with_clock(1_787_161_000_000);
        let written = store.append(NewEvent::new("a")).unwrap();
        assert_eq!(written.ts_ms, 1_787_161_000_000);
    }

    // The ordinary case. A healthy box's rows must stay byte-identical to what
    // they were before any of this existed, so the marker's presence is itself
    // the signal.
    #[test]
    fn a_trusted_clock_leaves_the_payload_untouched() {
        let (store, clock) = store_with_clock(1_787_161_000_000);
        clock.set_synced(true);

        let written = store.append(NewEvent::new("a")).unwrap();
        assert_eq!(written.payload, serde_json::json!({}));
    }

    #[test]
    fn an_untrusted_clock_marks_the_row() {
        let (store, _clock) = store_with_clock(1_787_161_000_000);
        let written = store.append(NewEvent::new("a")).unwrap();
        assert_eq!(written.payload[CLOCK_SYNCED], false);
    }

    #[test]
    fn the_mark_sits_alongside_whatever_the_caller_recorded() {
        let (store, _clock) = store_with_clock(1_787_161_000_000);
        let written = store
            .append(NewEvent::new("app.exited").payload(serde_json::json!({
                "exit_observed": false,
                "last_alive_ms": 1_787_161_900_000i64,
            })))
            .unwrap();

        assert_eq!(written.payload[CLOCK_SYNCED], false);
        assert_eq!(written.payload["exit_observed"], false);
        assert_eq!(written.payload["last_alive_ms"], 1_787_161_900_000i64);
    }

    #[test]
    fn the_mark_survives_a_round_trip_through_the_database() {
        let (store, _clock) = store_with_clock(1_787_161_000_000);
        store.append(NewEvent::new("a")).unwrap();

        let read = store.read_since(0, -1).unwrap();
        assert_eq!(read[0].payload[CLOCK_SYNCED], false);
    }

    // Once the clock is trusted the marking stops, within one store's lifetime.
    #[test]
    fn rows_written_after_the_clock_syncs_are_unmarked() {
        let (store, clock) = store_with_clock(1_787_161_000_000);
        let before = store.append(NewEvent::new("before")).unwrap();

        clock.set_synced(true);
        clock.set_wall_ms(1_787_161_808_000);
        let after = store.append(NewEvent::new("after")).unwrap();

        assert_eq!(before.payload[CLOCK_SYNCED], false);
        assert_eq!(after.payload, serde_json::json!({}));
        assert_eq!(after.ts_ms, 1_787_161_808_000);
    }

    // A caller that has already answered the question owns the answer. The
    // store adds; it never overwrites.
    #[test]
    fn the_store_does_not_clobber_a_mark_the_caller_set() {
        let (store, _clock) = store_with_clock(1_787_161_000_000);
        let written = store
            .append(NewEvent::new("a").payload(serde_json::json!({ CLOCK_SYNCED: true })))
            .unwrap();
        assert_eq!(written.payload[CLOCK_SYNCED], true);
    }

    // `NewEvent::payload` takes any Value. Nothing in SESH sets a non-object
    // today, but the store must not panic if something does.
    #[test]
    fn a_payload_that_is_not_an_object_is_left_as_it_is() {
        let (store, _clock) = store_with_clock(1_787_161_000_000);
        let written = store
            .append(NewEvent::new("a").payload(serde_json::json!("just a string")))
            .unwrap();
        assert_eq!(written.payload, serde_json::json!("just a string"));
    }

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
