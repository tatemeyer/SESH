//! The shared music queue, derived entirely from the log.
//!
//! SESH keeps the authoritative queue here and hands Spotify exactly one track
//! at a time. That is the decision the whole arc turns on: the Web API can add
//! to Spotify's queue but cannot reorder it or remove from it, so pushing the
//! whole queue would make veto impossible — once a track is in Spotify's queue
//! it is going to play.
//!
//! A pleasing consequence of being a projection: **the queue survives a
//! `seshd` restart**, which Spotify's own queue does not.
//!
//! ## Entries are keyed by event id, not track URI
//!
//! `subject` carries the track URI so the log reads naturally, but a queue
//! entry's identity is `payload.entry` — the id of the `music.queued` event
//! that created it. Two people queueing the same song is not an edge case on a
//! shared-queue night, and keying on the URI would silently merge their two
//! entries into one, so vetoing either would kill both.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::event::{kind, Event};
use crate::projection::Projection;

/// One track in the queue, or the one playing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Id of the `music.queued` event that created this entry. Stable, unique
    /// even when the same track is queued twice, and assigned by the log.
    pub entry: i64,
    /// Spotify track URI.
    pub uri: String,
    /// Track title, as it was when queued.
    pub title: String,
    /// Artist, as it was when queued.
    pub artist: String,
    /// Length in milliseconds. Zero when unknown.
    pub duration_ms: i64,
    /// Who added it. `None` for a track SESH did not queue itself.
    pub added_by: Option<String>,
    /// Who has voted to skip it, deduplicated by person.
    pub vetoes: BTreeSet<String>,
}

/// What is playing and what is waiting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Queue {
    pending: Vec<Entry>,
    now_playing: Option<Entry>,
}

impl Queue {
    /// Tracks waiting, in the order they were added.
    pub fn pending(&self) -> &[Entry] {
        &self.pending
    }

    /// What is playing, if anything.
    pub fn now_playing(&self) -> Option<&Entry> {
        self.now_playing.as_ref()
    }

    /// The track the conductor should hand Spotify next.
    pub fn next_up(&self) -> Option<&Entry> {
        self.pending.first()
    }

    /// Find an entry by id, whether it is waiting or playing.
    pub fn find(&self, entry: i64) -> Option<&Entry> {
        self.pending
            .iter()
            .chain(self.now_playing.iter())
            .find(|candidate| candidate.entry == entry)
    }
}

impl Queue {
    /// Remove a waiting entry by id and hand it back.
    fn take_pending(&mut self, entry: i64) -> Option<Entry> {
        let at = self
            .pending
            .iter()
            .position(|candidate| candidate.entry == entry)?;
        Some(self.pending.remove(at))
    }

    fn find_mut(&mut self, entry: i64) -> Option<&mut Entry> {
        self.pending
            .iter_mut()
            .chain(self.now_playing.iter_mut())
            .find(|candidate| candidate.entry == entry)
    }
}

impl Entry {
    /// Build an entry from the event that introduced the track.
    ///
    /// Metadata is best-effort: a track with no title still plays, and a
    /// projection that refused to fold a thin event would make the log
    /// unrebuildable — the one thing it must never be.
    fn from_event(event: &Event, uri: &str, entry: i64, added_by: Option<String>) -> Self {
        let text = |key: &str| event.payload[key].as_str().unwrap_or_default().to_string();
        Self {
            entry,
            uri: uri.to_string(),
            title: text("title"),
            artist: text("artist"),
            duration_ms: event.payload["duration_ms"].as_i64().unwrap_or(0),
            added_by,
            vetoes: BTreeSet::new(),
        }
    }
}

/// The queue entry a music event refers to.
fn entry_id(event: &Event) -> Option<i64> {
    event.payload["entry"].as_i64()
}

impl Projection for Queue {
    fn apply(&mut self, event: &Event) {
        match event.kind.as_str() {
            kind::MUSIC_QUEUED => {
                // No track means nothing to play. The entry id is this event's
                // own id, which is what makes two copies of one song distinct.
                let Some(uri) = event.subject.as_deref() else {
                    return;
                };
                let added_by = event.actors.first().cloned();
                self.pending
                    .push(Entry::from_event(event, uri, event.id, added_by));
            }

            kind::MUSIC_STARTED => {
                let referenced = entry_id(event);
                let promoted = referenced.and_then(|entry| self.take_pending(entry));

                // Falling back to the event itself is what lets Phase 4
                // reconcile against Spotify: someone pressed play in the
                // Spotify app, so a track is playing that SESH never queued,
                // and the log should say so rather than pretend otherwise.
                let starting = promoted.or_else(|| {
                    let uri = event.subject.as_deref()?;
                    Some(Entry::from_event(
                        event,
                        uri,
                        referenced.unwrap_or(event.id),
                        None,
                    ))
                });

                // Only replace what is playing if this event actually named a
                // track; a malformed event must not silently empty the card.
                if let Some(starting) = starting {
                    self.now_playing = Some(starting);
                }
            }

            // One rule for both "vetoed before it ever played" and "skipped
            // while playing", which is why there is no second kind for drops.
            kind::MUSIC_SKIPPED => {
                let Some(entry) = entry_id(event) else {
                    return;
                };
                if self
                    .now_playing
                    .as_ref()
                    .is_some_and(|playing| playing.entry == entry)
                {
                    self.now_playing = None;
                } else {
                    self.take_pending(entry);
                }
            }

            kind::MUSIC_VETOED => {
                let (Some(entry), Some(voter)) = (entry_id(event), event.actors.first()) else {
                    return;
                };
                // A veto naming an entry that is already gone is dropped: the
                // vote raced the skip it was asking for and simply lost.
                if let Some(target) = self.find_mut(entry) {
                    target.vetoes.insert(voter.clone());
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build an event the way the store would have stored it.
    fn ev(id: i64, kind: &str, subject: Option<&str>, payload: serde_json::Value) -> Event {
        Event {
            id,
            ts_ms: 1_786_937_604_000 + id,
            kind: kind.into(),
            actors: Vec::new(),
            subject: subject.map(str::to_string),
            payload,
        }
    }

    fn by(mut event: Event, actor: &str) -> Event {
        event.actors.push(actor.into());
        event
    }

    fn queued(id: i64, uri: &str, title: &str, actor: &str) -> Event {
        by(
            ev(
                id,
                kind::MUSIC_QUEUED,
                Some(uri),
                json!({ "title": title, "artist": "Someone", "duration_ms": 210_000 }),
            ),
            actor,
        )
    }

    fn started(id: i64, uri: &str, entry: i64) -> Event {
        ev(
            id,
            kind::MUSIC_STARTED,
            Some(uri),
            json!({ "entry": entry }),
        )
    }

    fn skipped(id: i64, uri: &str, entry: i64, why: &str) -> Event {
        ev(
            id,
            kind::MUSIC_SKIPPED,
            Some(uri),
            json!({ "entry": entry, "why": why }),
        )
    }

    fn vetoed(id: i64, uri: &str, entry: i64, actor: &str) -> Event {
        by(
            ev(id, kind::MUSIC_VETOED, Some(uri), json!({ "entry": entry })),
            actor,
        )
    }

    fn uris(queue: &Queue) -> Vec<&str> {
        queue.pending().iter().map(|e| e.uri.as_str()).collect()
    }

    #[test]
    fn an_empty_log_is_an_empty_queue() {
        let queue = Queue::rebuild(&[]);
        assert!(queue.pending().is_empty());
        assert_eq!(queue.now_playing(), None);
        assert_eq!(queue.next_up(), None);
    }

    #[test]
    fn queueing_keeps_insertion_order() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:b", "B", "marcus"),
            queued(3, "spotify:track:c", "C", "sam"),
        ]);
        assert_eq!(
            uris(&queue),
            vec!["spotify:track:a", "spotify:track:b", "spotify:track:c"]
        );
        assert_eq!(queue.next_up().unwrap().uri, "spotify:track:a");
    }

    #[test]
    fn an_entry_carries_its_metadata_and_who_added_it() {
        let queue = Queue::rebuild(&[queued(1, "spotify:track:a", "Song", "sam")]);
        let entry = &queue.pending()[0];

        assert_eq!(entry.entry, 1, "the entry id is the queued event's id");
        assert_eq!(entry.title, "Song");
        assert_eq!(entry.artist, "Someone");
        assert_eq!(entry.duration_ms, 210_000);
        assert_eq!(entry.added_by.as_deref(), Some("sam"));
        assert!(entry.vetoes.is_empty());
    }

    // The reason entries are keyed by event id. Two people queue the same song
    // — vetoing one must not touch the other.
    #[test]
    fn the_same_track_queued_twice_is_two_independent_entries() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:a", "A", "marcus"),
            vetoed(3, "spotify:track:a", 1, "ali"),
        ]);

        assert_eq!(queue.pending().len(), 2);
        assert_eq!(queue.find(1).unwrap().vetoes.len(), 1);
        assert!(
            queue.find(2).unwrap().vetoes.is_empty(),
            "vetoing one entry must not veto the other"
        );
    }

    // The discriminating half of the case above, and it has to name the
    // *second* copy. Vetoing the first proves nothing: an implementation that
    // wrongly looked entries up by URI would find the first one either way and
    // pass. Found by mutation testing, which is the only reason this exists.
    #[test]
    fn vetoing_the_second_copy_of_a_song_leaves_the_first_alone() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:a", "A", "marcus"),
            vetoed(3, "spotify:track:a", 2, "ali"),
        ]);

        assert!(
            queue.find(1).unwrap().vetoes.is_empty(),
            "the first copy must be untouched"
        );
        assert_eq!(queue.find(2).unwrap().vetoes.len(), 1);
    }

    // Likewise: removing whatever happens to be at the front of the queue
    // looks correct until the named entry is not at the front.
    #[test]
    fn starting_a_track_from_the_middle_leaves_the_others_in_order() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:b", "B", "sam"),
            queued(3, "spotify:track:c", "C", "sam"),
            started(4, "spotify:track:b", 2),
        ]);

        assert_eq!(queue.now_playing().unwrap().uri, "spotify:track:b");
        assert_eq!(uris(&queue), vec!["spotify:track:a", "spotify:track:c"]);
    }

    #[test]
    fn skipping_a_track_from_the_middle_leaves_the_others_in_order() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:b", "B", "sam"),
            queued(3, "spotify:track:c", "C", "sam"),
            skipped(4, "spotify:track:b", 2, "vetoed"),
        ]);

        assert_eq!(uris(&queue), vec!["spotify:track:a", "spotify:track:c"]);
    }

    #[test]
    fn a_person_voting_twice_counts_once() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            vetoed(2, "spotify:track:a", 1, "marcus"),
            vetoed(3, "spotify:track:a", 1, "marcus"),
        ]);
        assert_eq!(queue.find(1).unwrap().vetoes.len(), 1);
    }

    #[test]
    fn starting_a_track_moves_it_out_of_pending() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:b", "B", "sam"),
            started(3, "spotify:track:a", 1),
        ]);

        assert_eq!(queue.now_playing().unwrap().entry, 1);
        assert_eq!(uris(&queue), vec!["spotify:track:b"]);
    }

    #[test]
    fn a_veto_carries_over_when_the_track_starts() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            vetoed(2, "spotify:track:a", 1, "marcus"),
            started(3, "spotify:track:a", 1),
        ]);
        assert_eq!(queue.now_playing().unwrap().vetoes.len(), 1);
    }

    #[test]
    fn the_playing_track_can_be_vetoed() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            started(2, "spotify:track:a", 1),
            vetoed(3, "spotify:track:a", 1, "marcus"),
        ]);
        assert_eq!(queue.now_playing().unwrap().vetoes.len(), 1);
    }

    #[test]
    fn skipping_the_playing_track_clears_it() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            started(2, "spotify:track:a", 1),
            skipped(3, "spotify:track:a", 1, "finished"),
        ]);
        assert_eq!(queue.now_playing(), None);
        assert!(queue.pending().is_empty());
    }

    // One rule covers both "vetoed before it ever played" and "skipped while
    // playing", which is why `music.skipped` does not need two kinds.
    #[test]
    fn skipping_a_waiting_track_drops_it_and_leaves_the_playing_one() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:b", "B", "sam"),
            started(3, "spotify:track:a", 1),
            skipped(4, "spotify:track:b", 2, "vetoed"),
        ]);

        assert_eq!(queue.now_playing().unwrap().entry, 1);
        assert!(queue.pending().is_empty());
    }

    // Phase 4 reconciles against Spotify rather than the log, so a track can
    // start that SESH never queued — someone pressed play in the Spotify app.
    #[test]
    fn a_track_started_outside_sesh_still_becomes_now_playing() {
        let queue = Queue::rebuild(&[ev(
            1,
            kind::MUSIC_STARTED,
            Some("spotify:track:z"),
            json!({ "title": "Elsewhere", "artist": "Nobody" }),
        )]);

        let playing = queue.now_playing().expect("must reflect reality");
        assert_eq!(playing.uri, "spotify:track:z");
        assert_eq!(playing.title, "Elsewhere");
        assert_eq!(playing.added_by, None, "nobody here queued it");
    }

    #[test]
    fn a_veto_for_an_entry_that_no_longer_exists_is_ignored() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            skipped(2, "spotify:track:a", 1, "vetoed"),
            vetoed(3, "spotify:track:a", 1, "marcus"),
        ]);
        assert!(queue.pending().is_empty());
        assert_eq!(queue.now_playing(), None);
    }

    #[test]
    fn a_veto_with_no_voter_is_ignored() {
        let queue = Queue::rebuild(&[
            queued(1, "spotify:track:a", "A", "sam"),
            ev(
                2,
                kind::MUSIC_VETOED,
                Some("spotify:track:a"),
                json!({ "entry": 1 }),
            ),
        ]);
        assert!(queue.find(1).unwrap().vetoes.is_empty());
    }

    #[test]
    fn a_queued_event_with_no_track_is_ignored() {
        let queue = Queue::rebuild(&[ev(1, kind::MUSIC_QUEUED, None, json!({}))]);
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn unrelated_events_are_ignored() {
        let queue = Queue::rebuild(&[
            ev(1, kind::APP_LAUNCHED, Some("kodi"), json!({})),
            queued(2, "spotify:track:a", "A", "sam"),
            ev(3, kind::PRESENCE_ARRIVED, None, json!({})),
        ]);
        assert_eq!(queue.pending().len(), 1);
    }

    #[test]
    fn missing_metadata_degrades_rather_than_failing() {
        let queue = Queue::rebuild(&[ev(
            1,
            kind::MUSIC_QUEUED,
            Some("spotify:track:a"),
            json!({}),
        )]);
        let entry = &queue.pending()[0];
        assert_eq!(entry.title, "");
        assert_eq!(entry.duration_ms, 0);
    }

    #[test]
    fn incremental_apply_matches_a_full_rebuild() {
        let events = vec![
            queued(1, "spotify:track:a", "A", "sam"),
            queued(2, "spotify:track:b", "B", "marcus"),
            vetoed(3, "spotify:track:b", 2, "sam"),
            started(4, "spotify:track:a", 1),
            skipped(5, "spotify:track:a", 1, "finished"),
            started(6, "spotify:track:b", 2),
        ];

        let mut incremental = Queue::default();
        for event in &events {
            incremental.apply(event);
        }

        assert_eq!(incremental.now_playing().unwrap().entry, 2);
        assert!(incremental.pending().is_empty());
        assert_eq!(incremental, Queue::rebuild(&events));
    }
}
