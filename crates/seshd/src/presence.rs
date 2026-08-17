//! Who is in the room, inferred from phones.
//!
//! Veto needs a denominator — a majority *of whom*. Rather than invent a
//! second notion of who is here, phones heartbeat and this tracker turns those
//! beats into the same `presence.arrived` / `presence.left` events the Arc 1
//! [`Roster`](crate::projections::roster::Roster) projection already folds. The
//! vision anticipated exactly this as the degraded mode: *"presence dies →
//! roster falls back to whoever is on the phone app."*
//!
//! Events are emitted on **transitions only**. A phone beating every few
//! seconds all evening produces two rows, not hundreds — the log records that
//! someone was here, not that they were still here, again, and again.
//!
//! When BLE presence lands in a later arc it becomes a second producer of the
//! same two kinds, and nothing downstream has to change.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::event::{kind, NewEvent};
use crate::room::Room;
use crate::store::now_ms;

/// How long a phone may go quiet before its owner is considered gone.
///
/// Generous on purpose. A phone that locks its screen, drops to a dozing
/// radio, or loses Wi-Fi for a minute has not left the room, and a roster that
/// flaps would make veto thresholds move under people mid-vote.
pub const WINDOW_MS: i64 = 10 * 60 * 1000;

#[derive(Debug, Clone)]
struct Seen {
    last_ms: i64,
    present: bool,
}

/// Last-seen state per person, and the transitions it implies.
#[derive(Debug, Default)]
pub struct Presence {
    seen: Mutex<BTreeMap<String, Seen>>,
}

impl Presence {
    /// An empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// A tracker that already believes `present` are here.
    ///
    /// Used at startup. The log — and so the rebuilt roster — may already say
    /// people are in the room; without seeding, the first heartbeat after a
    /// restart would announce an arrival for someone who never left, and the
    /// log would collect a spurious `presence.arrived` on every daemon
    /// restart. Seeded people are still swept normally if they never beat.
    pub fn seeded(present: &[String], now_ms: i64) -> Self {
        let seen = present
            .iter()
            .map(|person| {
                (
                    person.clone(),
                    Seen {
                        last_ms: now_ms,
                        present: true,
                    },
                )
            })
            .collect();

        Self {
            seen: Mutex::new(seen),
        }
    }

    /// Record a heartbeat, returning an event only if this is an arrival.
    pub fn beat(&self, person: &str, now_ms: i64) -> Option<NewEvent> {
        let mut seen = self.seen.lock().expect("presence mutex poisoned");

        let entry = seen.entry(person.to_string()).or_insert(Seen {
            last_ms: now_ms,
            present: false,
        });
        entry.last_ms = now_ms;

        // Presence is held explicitly rather than inferred from `last_ms`, so
        // arrival and departure cannot both fire for one transition however
        // the sweep and the beat interleave.
        if entry.present {
            return None;
        }
        entry.present = true;
        Some(NewEvent::new(kind::PRESENCE_ARRIVED).actor(person))
    }

    /// Retire anyone who has gone quiet, returning their departures.
    pub fn sweep(&self, now_ms: i64) -> Vec<NewEvent> {
        let mut seen = self.seen.lock().expect("presence mutex poisoned");

        // BTreeMap, so the order is the people's ids — deterministic, which
        // keeps the log's ordering reproducible rather than hash-dependent.
        seen.iter_mut()
            .filter(|(_, state)| state.present && now_ms - state.last_ms >= WINDOW_MS)
            .map(|(person, state)| {
                state.present = false;
                NewEvent::new(kind::PRESENCE_LEFT).actor(person)
            })
            .collect()
    }
}

/// How often to look for phones that have gone quiet.
///
/// Far shorter than [`WINDOW_MS`], so a departure is recorded within a minute
/// of becoming true rather than up to ten minutes late.
pub const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Retire quiet phones forever, recording each departure.
///
/// Shaped like `Launcher::reap_loop`: the timing lives in the library next to
/// the logic it drives, so `main` stays a wiring file.
pub async fn sweep_loop(presence: Arc<Presence>, room: Arc<Room>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        ticker.tick().await;

        for departure in presence.sweep(now_ms()) {
            let who = departure.actors.first().cloned().unwrap_or_default();
            let Err(error) = room.record(departure) else {
                continue;
            };
            tracing::error!(%error, %who, "recording presence.left failed");

            // `sweep` has already marked them absent, but the log does not
            // know it. Put them back so the two agree — otherwise this
            // departure is lost for good, since a second sweep will not
            // re-emit a transition it believes it already made. They are
            // retired again one window later if they really have gone.
            let _ = presence.beat(&who, now_ms());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_786_937_604_000;

    fn kinds(events: &[NewEvent]) -> Vec<&str> {
        events.iter().map(|e| e.kind.as_str()).collect()
    }

    #[test]
    fn a_first_beat_announces_an_arrival() {
        let presence = Presence::new();
        let event = presence.beat("sam", T0).expect("first beat is an arrival");

        assert_eq!(event.kind, kind::PRESENCE_ARRIVED);
        assert_eq!(event.actors, vec!["sam".to_string()]);
    }

    #[test]
    fn beating_again_inside_the_window_announces_nothing() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();

        assert!(presence.beat("sam", T0 + 1_000).is_none());
        assert!(presence.beat("sam", T0 + WINDOW_MS - 1).is_none());
    }

    #[test]
    fn sweeping_inside_the_window_retires_nobody() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();

        assert!(presence.sweep(T0 + WINDOW_MS - 1).is_empty());
    }

    #[test]
    fn a_quiet_phone_is_retired_after_the_window() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();

        let left = presence.sweep(T0 + WINDOW_MS);
        assert_eq!(kinds(&left), vec![kind::PRESENCE_LEFT]);
        assert_eq!(left[0].actors, vec!["sam".to_string()]);
    }

    // Transitions only. The sweep runs every minute forever; it must not
    // append a departure every time it notices the same absent person.
    #[test]
    fn sweeping_twice_retires_someone_only_once() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();

        assert_eq!(presence.sweep(T0 + WINDOW_MS).len(), 1);
        assert!(presence.sweep(T0 + WINDOW_MS + 1).is_empty());
        assert!(presence.sweep(T0 + WINDOW_MS * 5).is_empty());
    }

    #[test]
    fn coming_back_after_being_retired_announces_a_new_arrival() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();
        presence.sweep(T0 + WINDOW_MS);

        let back = presence.beat("sam", T0 + WINDOW_MS + 1);
        assert_eq!(
            back.map(|e| e.kind),
            Some(kind::PRESENCE_ARRIVED.to_string())
        );
    }

    #[test]
    fn people_are_tracked_independently() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();
        presence.beat("marcus", T0 + WINDOW_MS / 2).unwrap();

        // Sam has gone quiet; Marcus beat more recently and stays.
        let left = presence.sweep(T0 + WINDOW_MS);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].actors, vec!["sam".to_string()]);

        assert!(presence.beat("marcus", T0 + WINDOW_MS).is_none());
    }

    #[test]
    fn a_seeded_person_does_not_re_announce_on_their_first_beat() {
        let presence = Presence::seeded(&["sam".to_string()], T0);

        assert!(
            presence.beat("sam", T0 + 1_000).is_none(),
            "a restart must not append an arrival for someone who never left"
        );
    }

    #[test]
    fn a_seeded_person_who_never_beats_is_still_retired() {
        let presence = Presence::seeded(&["sam".to_string()], T0);

        let left = presence.sweep(T0 + WINDOW_MS);
        assert_eq!(kinds(&left), vec![kind::PRESENCE_LEFT]);
    }

    #[test]
    fn sweeping_an_empty_tracker_is_fine() {
        assert!(Presence::new().sweep(T0).is_empty());
    }

    #[test]
    fn departures_are_returned_in_a_stable_order() {
        let presence = Presence::new();
        presence.beat("sam", T0).unwrap();
        presence.beat("marcus", T0).unwrap();
        presence.beat("ali", T0).unwrap();

        let actors: Vec<_> = presence
            .sweep(T0 + WINDOW_MS)
            .into_iter()
            .map(|e| e.actors[0].clone())
            .collect();
        assert_eq!(actors, vec!["ali", "marcus", "sam"]);
    }
}
