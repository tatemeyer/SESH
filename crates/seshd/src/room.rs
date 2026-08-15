//! The Room: the seam between the event log and everything that reads it.
//!
//! Every write to SESH goes through `Room::record`, which appends to the
//! log, folds the event into live projections, and fans it out to
//! subscribers. Nothing else may write to the store.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::broadcast;

use crate::event::{Event, NewEvent};
use crate::projection::Projection;
use crate::projections::roster::Roster;
use crate::store::Store;

/// Broadcast backlog. A subscriber that falls this far behind is lagged,
/// and reconnects with `GET /api/events?after=<last_id>` to catch up.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// The live room: the event log plus every view derived from it.
pub struct Room {
    store: Store,
    events_tx: broadcast::Sender<Event>,
    roster: Mutex<Roster>,
}

impl Room {
    /// Open a room over `store`, restoring projections from the log.
    pub fn new(store: Store) -> Result<Arc<Self>> {
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let history = store.read_since(0, -1)?;
        let roster = Roster::rebuild(&history);

        Ok(Arc::new(Self {
            store,
            events_tx,
            roster: Mutex::new(roster),
        }))
    }

    /// Append an event, update projections, and fan it out. The only write path.
    pub fn record(&self, new: NewEvent) -> Result<Event> {
        let event = self.store.append(new)?;
        self.roster
            .lock()
            .expect("roster mutex poisoned")
            .apply(&event);
        // Fails only when there are no subscribers, which is not an error.
        let _ = self.events_tx.send(event.clone());
        Ok(event)
    }

    /// Subscribe to the live event feed.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    /// Person ids currently in the room.
    pub fn roster(&self) -> Vec<String> {
        self.roster.lock().expect("roster mutex poisoned").present()
    }

    /// Read history. Pass `limit = -1` for no limit.
    pub fn events_since(&self, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        self.store.read_since(after_id, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::kind;

    fn room() -> Arc<Room> {
        Room::new(Store::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn recording_persists_the_event() {
        let room = room();
        room.record(NewEvent::new("a")).unwrap();
        room.record(NewEvent::new("b")).unwrap();

        let events = room.events_since(0, -1).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn recording_updates_the_roster() {
        let room = room();
        assert!(room.roster().is_empty());

        room.record(NewEvent::new(kind::PRESENCE_ARRIVED).actor("tate"))
            .unwrap();
        assert_eq!(room.roster(), vec!["tate".to_string()]);

        room.record(NewEvent::new(kind::PRESENCE_LEFT).actor("tate"))
            .unwrap();
        assert!(room.roster().is_empty());
    }

    #[tokio::test]
    async fn subscribers_receive_recorded_events() {
        let room = room();
        let mut rx = room.subscribe();

        let written = room
            .record(NewEvent::new("moment.captured").subject("clip-1"))
            .unwrap();
        let received = rx.recv().await.unwrap();

        assert_eq!(received, written);
    }

    #[test]
    fn projections_are_restored_from_the_log_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        {
            let room = Room::new(Store::open(&path).unwrap()).unwrap();
            room.record(NewEvent::new(kind::PRESENCE_ARRIVED).actor("sam"))
                .unwrap();
        }

        let reopened = Room::new(Store::open(&path).unwrap()).unwrap();
        assert_eq!(reopened.roster(), vec!["sam".to_string()]);
    }
}
