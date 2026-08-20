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

pub mod fusion;
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

    /// Who is looking at SESH right now.
    ///
    /// The right input for *may this person act* — a question about the phone
    /// in someone's hand. **Never the veto denominator**: a majority of the
    /// people currently staring at their phones is not a majority of the room,
    /// and using it there is the bug this arc is named after.
    pub fn attentive(&self, now_ms: i64) -> Vec<String> {
        let seen = self.seen.lock().expect("presence mutex poisoned");
        seen.iter()
            .filter(|(_, state)| state.present && now_ms - state.last_ms < ATTENTION_MS)
            .map(|(person, _)| person.clone())
            .collect()
    }

    /// Who this tracker believes is in the room.
    ///
    /// The heartbeat's answer to the presence question, and only ever one
    /// input to it. The authoritative roster is rebuilt from the log — see
    /// [`Room::roster`](crate::room::Room::roster) — because this tracker
    /// knows about phones and the room is not made of phones.
    pub fn present(&self, now_ms: i64) -> Vec<String> {
        let seen = self.seen.lock().expect("presence mutex poisoned");
        seen.iter()
            .filter(|(_, state)| state.present && now_ms - state.last_ms < WINDOW_MS)
            .map(|(person, _)| person.clone())
            .collect()
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

/// How long since a phone last beat before its owner stops counting as
/// *paying attention*.
///
/// Deliberately a small fraction of [`WINDOW_MS`], because these answer
/// different questions. Attention is "is this person looking at SESH right
/// now", and a locked screen ends it within seconds. Presence is "is this
/// person in the room tonight", and a locked screen says nothing about it at
/// all — which is the whole defect this arc exists to fix.
pub const ATTENTION_MS: i64 = 90 * 1000;

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
mod tests;

#[cfg(test)]
#[path = "fusion_tests.rs"]
mod fusion_tests;
