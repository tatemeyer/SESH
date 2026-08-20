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
//! When BLE presence lands it becomes a second producer of the same two kinds,
//! and nothing downstream has to change. What does change is that the row must
//! then say *which* producer it came from — see [`via`], and note that this
//! tracker is only ever the `heartbeat` one.

pub mod via;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::clock::Clock;
use crate::event::{kind, NewEvent};
use crate::room::Room;
use crate::presence::via::{Via, VIA};

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
        Some(
            NewEvent::new(kind::PRESENCE_ARRIVED)
                .actor(person)
                .payload(json!({ VIA: Via::Heartbeat })),
        )
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
                NewEvent::new(kind::PRESENCE_LEFT)
                    .actor(person)
                    .payload(json!({ VIA: Via::Heartbeat }))
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
///
/// Times from [`Clock::mono_ms`]. [`WINDOW_MS`] is a duration, and a wall clock
/// that steps forward further than ten minutes — which is every cold boot on a
/// Pi with no RTC — would retire every phone in the room at once and write a
/// `presence.left` for each of them.
pub async fn sweep_loop(presence: Arc<Presence>, room: Arc<Room>, clock: Arc<dyn Clock>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        ticker.tick().await;

        for departure in presence.sweep(clock.mono_ms()) {
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
            let _ = presence.beat(&who, clock.mono_ms());
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

    /// The behaviour change this module gained in Arc 3 Phase 1. Every row this
    /// tracker writes is a heartbeat row and must say so — it is the only
    /// producer that can honestly claim `heartbeat`, and once BLE and wifi write
    /// the same two kinds, a row that does not say is indistinguishable from a
    /// row written before anyone thought to ask.
    #[test]
    fn every_row_this_tracker_writes_says_it_came_from_a_heartbeat() {
        let presence = Presence::new();

        let arrival = presence.beat("sam", T0).expect("first beat is an arrival");
        assert_eq!(arrival.payload[VIA], "heartbeat");

        let departures = presence.sweep(T0 + WINDOW_MS);
        assert_eq!(kinds(&departures), vec![kind::PRESENCE_LEFT]);
        assert_eq!(departures[0].payload[VIA], "heartbeat");
    }

    /// Guards the seam the fusion projection will read across in Phase 3: what
    /// this tracker writes must survive `Via::read` as the real thing, not as
    /// `Other("heartbeat")` or as unknown.
    #[test]
    fn what_this_tracker_writes_reads_back_as_heartbeat() {
        let presence = Presence::new();
        let arrival = presence.beat("sam", T0).unwrap();

        let recorded = crate::event::Event {
            id: 1,
            ts_ms: T0,
            kind: arrival.kind.clone(),
            actors: arrival.actors.clone(),
            subject: arrival.subject.clone(),
            payload: arrival.payload.clone(),
        };
        assert_eq!(Via::read(&recorded), Some(Via::Heartbeat));
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
