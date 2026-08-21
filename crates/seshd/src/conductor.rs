//! The loop that makes the speaker agree with the queue.
//!
//! Everything above this module decides *what the room wants*; this decides
//! *when to tell the source about it*, and writes down what actually happened.
//!
//! ## It believes the speaker, not the log (D8)
//!
//! Arc 1's launcher reconciles the other way — it closes dangling
//! `app.launched` rows on startup, because restarting `seshd` kills the apps
//! it launched, so the log's claim is provably stale. **That argument does not
//! hold for music.** librespot is its own service outside `seshd`'s cgroup, so
//! after a restart the music is still playing. Here the source is the ground
//! truth and the log is corrected to match it, never the reverse.
//!
//! ## Nothing here is scheduled by wall-clock time
//!
//! [`Conductor::tick`] does one full pass and returns how long to wait before
//! the next. The waiting lives in [`run_loop`] alone, which is why every rule
//! below is testable by calling `tick` in a loop with no clock at all.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;

use crate::event::{kind, NewEvent};
use crate::player::{Playback, Player};
use crate::projections::queue::Queue;
use crate::room::Room;
use crate::veto;

/// How often to look while a track is playing.
pub const POLL_PLAYING: Duration = Duration::from_secs(3);

/// How often to look while the room is quiet.
pub const POLL_IDLE: Duration = Duration::from_secs(15);

/// How often to look while the source is unreachable.
pub const POLL_OFFLINE: Duration = Duration::from_secs(30);

/// How long before a track ends to hand the source its successor (D7).
///
/// Anything pushed is committed — Spotify's queue cannot be reordered or
/// emptied — so this window is exactly how long a veto can arrive too late.
/// Five seconds of that, against a gap of dead air between every single
/// track, is the trade the room is better off with.
pub const PREPUSH_MS: i64 = 5_000;

/// How far progress must jump backwards to count as a replay rather than a seek.
///
/// The only visible difference between "the same song, queued twice, second
/// copy now starting" and "somebody dragged the scrubber back" is this jump.
const REWIND_MS: i64 = 10_000;

/// Whether the music source is answering.
///
/// Shared with the API so `GET /api/music` can say `offline` rather than
/// quietly serving a queue that nothing is going to play.
#[derive(Debug, Default)]
pub struct Status {
    online: AtomicBool,
}

impl Status {
    /// A status that starts out unknown, which reads as offline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the last poll reached the source.
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// What to show a phone: `ok` or `offline`.
    pub fn label(&self) -> &'static str {
        if self.is_online() {
            "ok"
        } else {
            "offline"
        }
    }

    /// Record the outcome of a poll. Returns true when this changed things.
    fn set(&self, online: bool) -> bool {
        self.online.swap(online, Ordering::Relaxed) != online
    }
}

/// What the conductor has to remember between passes.
#[derive(Debug, Default)]
struct Inner {
    /// Entry handed to the source but not yet seen playing.
    ///
    /// This is the committed track of D7: it cannot be un-queued, so a veto
    /// against it is honoured late rather than pretended away.
    pushed: Option<i64>,
    /// Last progress seen, to catch a track restarting at the same URI.
    last_progress_ms: i64,
    /// A veto-skip that failed, and how many ticks to sit out before retrying.
    ///
    /// Without this the conductor asks the source to skip the same track on
    /// every tick for as long as it keeps failing. On 2026-08-20 that turned
    /// one broken skip into a rate-limited player: eight attempts in ninety
    /// seconds, then 429s, which take out search and playback state too. A
    /// veto that cannot be honoured is bad; a veto that cannot be honoured and
    /// disables the rest of the room is much worse.
    skip_backoff: u32,
}

/// Ticks to wait before retrying a veto-skip the source refused.
///
/// Long enough that a persistent failure costs a handful of calls a minute
/// rather than one per tick, short enough that a transient one still resolves
/// inside the track it was voted against.
const SKIP_BACKOFF_TICKS: u32 = 5;

/// Drives the music source from the queue.
pub struct Conductor {
    room: Arc<Room>,
    player: Arc<dyn Player>,
    status: Arc<Status>,
    state: Mutex<Inner>,
}

impl Conductor {
    /// Wire a conductor to a room and a source.
    pub fn new(room: Arc<Room>, player: Arc<dyn Player>, status: Arc<Status>) -> Arc<Self> {
        Arc::new(Self {
            room,
            player,
            status,
            state: Mutex::new(Inner::default()),
        })
    }

    /// One full pass. Returns how long to wait before the next.
    ///
    /// Ordered deliberately: make the log agree with the speaker first, then
    /// act on vetoes against that corrected picture, then decide what to hand
    /// over next. Acting before reconciling would vote on a stale queue.
    pub async fn tick(&self) -> Duration {
        let playback = match self.player.playback().await {
            Ok(playback) => {
                if self.status.set(true) {
                    tracing::info!("the music source is answering again");
                }
                playback
            }
            Err(error) => {
                // Once per transition, not once per poll: an unreachable
                // source every 30s for an evening would bury the log.
                if self.status.set(false) {
                    tracing::warn!(%error, "the music source is unreachable; queue still open");
                }
                return POLL_OFFLINE;
            }
        };

        let mut inner = self.state.lock().await;
        self.reconcile(&mut inner, playback.as_ref());
        self.honour_vetoes(&mut inner).await;
        self.hand_over_next(&mut inner, playback.as_ref()).await;

        match playback {
            Some(state) if state.is_playing => POLL_PLAYING,
            _ => POLL_IDLE,
        }
    }

    /// Make the log say what the speaker is actually doing (D8).
    fn reconcile(&self, inner: &mut Inner, playback: Option<&Playback>) {
        let queue = self.room.queue();
        let playing = queue.now_playing();

        let Some(state) = playback else {
            // Nothing active at all. Whatever the log thinks is playing has
            // ended; a pause reports as `Some` with `is_playing: false`, so
            // this is genuinely "the speaker is done".
            if let Some(old) = playing {
                self.record_skipped(old.entry, &old.uri, "finished");
            }
            inner.last_progress_ms = 0;
            return;
        };

        let same_uri = playing.map(|entry| entry.uri.as_str()) == Some(state.track.uri.as_str());
        // The same song queued twice (D1) changes nothing about the URI, so a
        // backwards jump in progress is the only evidence the second copy has
        // begun. Requiring a pushed entry keeps a scrubbed rewind from being
        // mistaken for one.
        let replayed = same_uri
            && inner.pushed.is_some()
            && state.progress_ms + REWIND_MS < inner.last_progress_ms;

        if !same_uri || replayed {
            if let Some(old) = playing {
                self.record_skipped(old.entry, &old.uri, "finished");
            }
            let entry = self.claim(&queue, inner, &state.track.uri);
            self.record_started(state, entry);
        }
        inner.last_progress_ms = state.progress_ms;
    }

    /// Which queue entry the track now playing corresponds to, if any.
    ///
    /// `None` means nobody here queued it — someone pressed play in the
    /// Spotify app. That is a legitimate thing to happen in a house, and the
    /// log records it rather than pretending the room is empty.
    fn claim(&self, queue: &Queue, inner: &mut Inner, uri: &str) -> Option<i64> {
        let pushed = inner
            .pushed
            .filter(|id| queue.find(*id).is_some_and(|entry| entry.uri == uri));

        // A cold start goes out through `play` rather than `pushed`, so fall
        // back to the first waiting copy of this URI.
        let claimed = pushed.or_else(|| {
            queue
                .pending()
                .iter()
                .find(|entry| entry.uri == uri)
                .map(|entry| entry.entry)
        })?;

        if inner.pushed == Some(claimed) {
            inner.pushed = None;
        }
        Some(claimed)
    }

    /// Skip what the room has voted out.
    async fn honour_vetoes(&self, inner: &mut Inner) {
        let queue = self.room.queue();
        let present = self.room.roster();

        if let Some(playing) = queue.now_playing() {
            if veto::should_skip(&playing.vetoes, &present) {
                // Sitting out a tick after a failure, so a source that is
                // refusing does not get hammered into rate-limiting us.
                if inner.skip_backoff > 0 {
                    inner.skip_backoff -= 1;
                    return;
                }
                // Tell the source first: recording a skip the speaker never
                // performed would leave the log lying about the room.
                if let Err(error) = self.player.skip().await {
                    inner.skip_backoff = SKIP_BACKOFF_TICKS;
                    tracing::warn!(
                        %error,
                        entry = playing.entry,
                        backoff_ticks = SKIP_BACKOFF_TICKS,
                        "could not skip a vetoed track; the room voted and the \
                         track is still playing"
                    );
                    return;
                }
                inner.skip_backoff = 0;
                self.record_skipped(playing.entry, &playing.uri, "vetoed");
                inner.last_progress_ms = 0;
            }
        }

        for entry in queue.pending() {
            // D7: this one is already committed to the source. Dropping it
            // here would take it out of SESH's queue while Spotify played it
            // anyway. It gets skipped the moment it starts instead.
            if inner.pushed == Some(entry.entry) {
                continue;
            }
            if veto::should_skip(&entry.vetoes, &present) {
                self.record_skipped(entry.entry, &entry.uri, "vetoed");
            }
        }
    }

    /// Pre-push the successor, or break the silence.
    async fn hand_over_next(&self, inner: &mut Inner, playback: Option<&Playback>) {
        let queue = self.room.queue();
        let Some(next) = queue.next_up() else {
            // An empty queue is a correct outcome. A room that fills its own
            // silence with recommendations is a different product.
            return;
        };

        match playback {
            // Near the end of something: hand over the successor so there is
            // no gap at the seam.
            Some(state) if state.is_playing && state.remaining_ms() <= PREPUSH_MS => {
                if inner.pushed == Some(next.entry) {
                    return;
                }
                match self.player.enqueue(&next.uri).await {
                    Ok(()) => inner.pushed = Some(next.entry),
                    Err(error) => tracing::warn!(%error, "could not pre-push the next track"),
                }
            }

            // Nothing on the speaker at all: start it. `enqueue` would append
            // to a queue nothing is draining, and the room would stay silent
            // with a full queue.
            None => match self.player.play(&next.uri).await {
                Ok(()) => inner.pushed = Some(next.entry),
                Err(error) => tracing::warn!(%error, "could not start the queue"),
            },

            _ => {}
        }
    }

    fn record_started(&self, state: &Playback, entry: Option<i64>) {
        let event = NewEvent::new(kind::MUSIC_STARTED)
            .subject(&state.track.uri)
            .payload(json!({
                "entry": entry,
                "title": state.track.title,
                "artist": state.track.artist,
                "duration_ms": state.track.duration_ms,
            }));
        if let Err(error) = self.room.record(event) {
            tracing::error!(%error, "recording music.started failed");
        }
    }

    fn record_skipped(&self, entry: i64, uri: &str, why: &str) {
        let event = NewEvent::new(kind::MUSIC_SKIPPED)
            .subject(uri)
            .payload(json!({ "entry": entry, "why": why }));
        if let Err(error) = self.room.record(event) {
            tracing::error!(%error, "recording music.skipped failed");
        }
    }
}

/// Poll forever, at whatever interval the last pass asked for.
pub async fn run_loop(conductor: Arc<Conductor>) {
    loop {
        let wait = conductor.tick().await;
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `set` returns whether anything changed, and that return value only ever
    // drives a log line — so nothing in the integration suite pins it. Invert
    // it and the Pi logs "the music source is answering again" on every poll
    // of a healthy source, while the one message that matters, the transition
    // to unreachable, never appears.
    #[test]
    fn a_status_reports_transitions_rather_than_state() {
        let status = Status::new();
        assert!(!status.is_online(), "unknown reads as offline");

        assert!(status.set(true), "offline -> online is a change");
        assert!(!status.set(true), "online -> online is not");
        assert!(status.is_online());

        assert!(status.set(false), "online -> offline is a change");
        assert!(!status.set(false), "offline -> offline is not");
        assert_eq!(status.label(), "offline");
    }
}
