//! Deciding who is in the room from several signals that disagree.
//!
//! [`Presence`](super::Presence) answers the question from one signal, the
//! heartbeat, because for two arcs there was only one. Once BLE, wifi and a
//! human's own say-so all produce [`Via`]s, something has to decide what the
//! room believes when they conflict — and they will, constantly: a phone locks
//! while its owner stays put, a tag is left in a coat, someone says "I'm here"
//! while every radio disagrees.
//!
//! Pure and clockless. Every method takes the time it should reason about, so
//! an entire evening can be replayed in a test with no hardware, no network and
//! no waiting. That is deliberate: BLE is the one part of this arc a test suite
//! cannot see, so everything that *can* be decided without a radio is.
//!
//! ## The rules
//!
//! **Presence is the union of live signals.** Any signal that has seen you
//! recently puts you in the room. This is the permissive direction on purpose —
//! failing to notice someone is a worse error than briefly believing in
//! somebody who stepped out, because the veto denominator is built on it and a
//! roster that flaps moves thresholds under people mid-vote.
//!
//! **A stronger signal overrides a weaker one only when it is *positively
//! absent*, never merely stale.** Positively absent means the room looked and
//! did not find you: a scan ran, and your tag was not in it. Stale means
//! nothing has been heard, which is not evidence — a scanner that has crashed
//! is silent in exactly the same way as a room you have left. Conflating those
//! two is how a broken radio empties a room full of people.
//!
//! **`asserted` outranks everything, and expires.** A human correcting the room
//! is the last word, because a room that cannot be told it is wrong becomes
//! infuriating. But an assertion that never expires is a lie with a long
//! half-life, so it ages out like every other signal.

use std::collections::BTreeMap;

use super::via::Via;

/// How long each signal's word is good for.
///
/// These differ because the signals fail differently, which is the entire
/// argument for keeping `via` on the row. A BLE gap of thirty seconds is
/// someone walking to the kitchen; a heartbeat gap of thirty seconds is a
/// locked screen; a wifi lease outlives both and knows only the flat.
pub fn window_ms(via: &Via) -> i64 {
    match via {
        // Generous relative to how often a tag advertises: a miss is common and
        // means almost nothing, several misses in a row mean you left.
        Via::Ble => 2 * 60 * 1000,
        // A lease is renewed rarely and lingers after a device sleeps.
        Via::Wifi => 15 * 60 * 1000,
        // Matches `super::WINDOW_MS` — a phone may doze for a long time without
        // its owner having gone anywhere.
        Via::Heartbeat => 10 * 60 * 1000,
        // One evening. Long enough that nobody re-asserts twice in a night,
        // short enough that it does not survive into tomorrow.
        Via::Asserted => 6 * 60 * 60 * 1000,
        // A producer this build does not know. Trust it as far as the weakest
        // thing we do know, rather than forever or not at all.
        Via::Other(_) => 10 * 60 * 1000,
    }
}

/// How much a signal's word is worth when two of them disagree.
///
/// Only ever compared, never displayed. Higher wins.
fn rank(via: &Via) -> u8 {
    match via {
        Via::Asserted => 4,
        Via::Ble => 3,
        Via::Wifi => 2,
        Via::Heartbeat => 1,
        Via::Other(_) => 0,
    }
}

/// What one signal last said about one person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// The signal saw them.
    Seen,
    /// The signal looked and did not find them.
    Missing,
}

#[derive(Debug, Clone)]
struct Said {
    word: Word,
    at_ms: i64,
}

/// Several signals' opinions about who is in the room, and the rules for
/// resolving them.
#[derive(Debug, Default)]
pub struct Fusion {
    said: BTreeMap<String, BTreeMap<String, Said>>,
}

impl Fusion {
    /// An empty view: nobody has been seen by anything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `via` saw `person` at `at_ms`.
    pub fn seen(&mut self, person: &str, via: &Via, at_ms: i64) {
        self.record(person, via, Word::Seen, at_ms);
    }

    /// Record that `via` looked for `person` at `at_ms` and did not find them.
    ///
    /// This is the strong statement, and only a signal that actually swept may
    /// make it. A scanner that has stopped running must **not** call this —
    /// silence is [`Word::Seen`] going stale, which is a different thing and is
    /// treated as such.
    pub fn missing(&mut self, person: &str, via: &Via, at_ms: i64) {
        self.record(person, via, Word::Missing, at_ms);
    }

    fn record(&mut self, person: &str, via: &Via, word: Word, at_ms: i64) {
        self.said
            .entry(person.to_string())
            .or_default()
            .insert(via.as_str().to_string(), Said { word, at_ms });
    }

    /// Whether the room believes `person` is in it at `now_ms`.
    pub fn is_present(&self, person: &str, now_ms: i64) -> bool {
        self.decide(person, now_ms).is_some()
    }

    /// Everyone the room believes is in it, in stable alphabetical order.
    pub fn present(&self, now_ms: i64) -> Vec<String> {
        self.said
            .keys()
            .filter(|person| self.is_present(person, now_ms))
            .cloned()
            .collect()
    }

    /// Which signal is currently the reason the room believes in `person`.
    ///
    /// `None` when the room does not believe in them at all. This is what a
    /// presence row records, so the log says not merely that someone is here
    /// but which sense the room is trusting.
    pub fn via_for(&self, person: &str, now_ms: i64) -> Option<Via> {
        self.decide(person, now_ms)
    }

    /// The rules, in one place.
    fn decide(&self, person: &str, now_ms: i64) -> Option<Via> {
        let words = self.said.get(person)?;

        // Only signals still inside their own window get a say. A stale signal
        // is not evidence of anything, in either direction.
        let live: Vec<(Via, &Said)> = words
            .iter()
            .map(|(raw, said)| (Via::from(raw.as_str()), said))
            .filter(|(via, said)| now_ms - said.at_ms < window_ms(via))
            .collect();

        // The strongest signal that positively looked and found nothing. Only a
        // signal at least this strong may still put the person in the room.
        let veto = live
            .iter()
            .filter(|(_, said)| said.word == Word::Missing)
            .map(|(via, _)| rank(via))
            .max();

        live.iter()
            .filter(|(_, said)| said.word == Word::Seen)
            .filter(|(via, _)| veto.is_none_or(|floor| rank(via) >= floor))
            .max_by_key(|(via, _)| rank(via))
            .map(|(via, _)| via.clone())
    }

    /// Forget everything older than `before_ms`, so an all-night session does
    /// not accumulate every person the room has ever seen.
    pub fn forget_before(&mut self, before_ms: i64) {
        for words in self.said.values_mut() {
            words.retain(|_, said| said.at_ms >= before_ms);
        }
        self.said.retain(|_, words| !words.is_empty());
    }
}
