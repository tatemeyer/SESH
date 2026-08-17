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
use crate::projections::queue::Queue;
use crate::projections::roster::Roster;
use crate::store::{Person, Store};

/// Broadcast backlog. A subscriber that falls this far behind is lagged.
/// Surfaces are level-triggered: a lagged or dropped client reconnects and
/// re-fetches current state rather than replaying the events it missed.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// The live room: the event log plus every view derived from it.
pub struct Room {
    store: Store,
    events_tx: broadcast::Sender<Event>,
    roster: Mutex<Roster>,
    queue: Mutex<Queue>,
    write: Mutex<()>,
}

impl Room {
    /// Open a room over `store`, restoring projections from the log.
    pub fn new(store: Store) -> Result<Arc<Self>> {
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let history = store.read_since(0, -1)?;
        let roster = Roster::rebuild(&history);
        // The queue rebuilds from the log for free, so it survives a restart —
        // which Spotify's own queue does not.
        let queue = Queue::rebuild(&history);

        Ok(Arc::new(Self {
            store,
            events_tx,
            roster: Mutex::new(roster),
            queue: Mutex::new(queue),
            write: Mutex::new(()),
        }))
    }

    /// Append an event, update projections, and fan it out. The only write path.
    pub fn record(&self, new: NewEvent) -> Result<Event> {
        // `Store::append` takes and drops the connection lock internally, so
        // without this guard two writers could append, then apply and publish
        // in the opposite order — breaking the one property the whole design
        // exists for: the live projection equals a rebuild from the log.
        //
        // Lock order is Launcher::current -> Room::write -> Store::conn ->
        // Room::roster -> Room::queue, never reversed. `record` is
        // synchronous, so no guard is ever held across an `.await`, and the
        // two projection guards are taken one after the other rather than
        // nested.
        let _write = self.write.lock().expect("write mutex poisoned");
        let event = self.store.append(new)?;
        self.roster
            .lock()
            .expect("roster mutex poisoned")
            .apply(&event);
        self.queue
            .lock()
            .expect("queue mutex poisoned")
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

    /// A snapshot of the music queue.
    ///
    /// Cloned rather than borrowed so no caller can hold the projection lock
    /// across an `.await` — a queue this size is a handful of small strings.
    pub fn queue(&self) -> Queue {
        self.queue.lock().expect("queue mutex poisoned").clone()
    }

    /// Read history. Pass `limit = -1` for no limit.
    pub fn events_since(&self, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        self.store.read_since(after_id, limit)
    }

    // The identity registry, reached through the Room rather than by handing
    // out the `Store`. Exposing the store would put `append` within reach of
    // every caller and quietly retire the "Room::record is the only write
    // path" invariant, so these delegate instead. `people` is not the log and
    // is not append-only — see `store::people`.

    /// Register a person and return them, allocating an id from their name.
    pub fn register_person(&self, name: &str, token: &str) -> Result<Person> {
        self.store.insert_person(name, token)
    }

    /// Resolve a phone token to whoever holds it.
    pub fn person_by_token(&self, token: &str) -> Result<Option<Person>> {
        self.store.person_by_token(token)
    }

    /// Everyone the house knows, oldest join first.
    pub fn people(&self) -> Result<Vec<Person>> {
        self.store.people()
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
    fn recording_music_events_updates_the_queue() {
        let room = room();
        assert!(room.queue().pending().is_empty());

        let queued = room
            .record(
                NewEvent::new(kind::MUSIC_QUEUED)
                    .actor("sam")
                    .subject("spotify:track:a")
                    .payload(serde_json::json!({ "title": "A" })),
            )
            .unwrap();

        let queue = room.queue();
        assert_eq!(queue.pending().len(), 1);
        assert_eq!(queue.pending()[0].entry, queued.id);
        assert_eq!(queue.pending()[0].added_by.as_deref(), Some("sam"));
    }

    // The queue is a projection, so it comes back from the log by itself.
    // Spotify's own queue does not survive a restart; this is the difference
    // that justifies SESH holding the authoritative one.
    #[test]
    fn the_queue_survives_reopening_the_room() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        {
            let room = Room::new(Store::open(&path).unwrap()).unwrap();
            room.record(
                NewEvent::new(kind::MUSIC_QUEUED)
                    .actor("sam")
                    .subject("spotify:track:a")
                    .payload(serde_json::json!({ "title": "A" })),
            )
            .unwrap();
        }

        let reopened = Room::new(Store::open(&path).unwrap()).unwrap();
        assert_eq!(reopened.queue().pending().len(), 1);
        assert_eq!(reopened.queue().pending()[0].title, "A");
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

    // The two tests below assert the invariant `Room::write` exists to hold.
    // They are invariant guards, not a reproduction: the unsynchronised window
    // between `Store::append` releasing the connection lock and `roster.apply`
    // is a few instructions wide, and removing the guard does not make either
    // test fail on demand. What enforces the property is the guard itself.
    fn record_concurrently(
        room: &Arc<Room>,
        threads: usize,
        each: impl Fn(&Room, usize) + Copy + Send + 'static,
    ) {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let room = room.clone();
                std::thread::spawn(move || each(&room, t))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn concurrent_records_reach_subscribers_in_log_order() {
        let room = room();
        let mut rx = room.subscribe();

        record_concurrently(&room, 8, |room, t| {
            for i in 0..8 {
                room.record(NewEvent::new(format!("k{t}-{i}"))).unwrap();
            }
        });

        let mut ids = Vec::new();
        while let Ok(event) = rx.try_recv() {
            ids.push(event.id);
        }

        assert_eq!(ids.len(), 64, "every record must reach the subscriber");
        let mut in_order = ids.clone();
        in_order.sort_unstable();
        assert_eq!(
            ids, in_order,
            "the bus must publish in the order the log assigned ids"
        );
    }

    #[test]
    fn concurrent_records_leave_the_roster_equal_to_a_rebuild() {
        let room = room();

        // Two writers toggling the *same* actor is the Arc 3 BLE-watcher
        // case: whichever order the log lands in, the cached roster has to
        // agree with a rebuild over that log.
        record_concurrently(&room, 4, |room, _| {
            for _ in 0..20 {
                room.record(NewEvent::new(kind::PRESENCE_ARRIVED).actor("tate"))
                    .unwrap();
                room.record(NewEvent::new(kind::PRESENCE_LEFT).actor("tate"))
                    .unwrap();
            }
        });

        let rebuilt = Roster::rebuild(&room.events_since(0, -1).unwrap());
        assert_eq!(
            room.roster(),
            rebuilt.present(),
            "the live projection must equal a rebuild from the log"
        );
    }
}
