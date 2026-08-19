//! The seam between SESH's queue and whatever actually makes sound.
//!
//! Shaped like [`Platform`](crate::launcher::platform::Platform) is for
//! process control, and for the same reason: everything above this line stays
//! testable with no network, no Spotify account, and no speaker. The conductor
//! in Phase 4 is written entirely against this trait.
//!
//! It is `async` rather than synchronous — unlike `Platform`, which is
//! synchronous because `std::process` is — because an HTTP client is. The
//! blocking `reqwest` client panics when it finds a Tokio runtime handle, and
//! `spawn_blocking` threads have one, so pretending otherwise would only move
//! the problem to runtime.

pub mod auth;
pub mod mock;
pub mod spotify;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A track that can be queued or played.
///
/// Deliberately thin. Album art, popularity, and the rest of Spotify's
/// metadata are not modelled: they would tie the log to one music source, and
/// February 2026 removed half of those endpoints anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// Source-specific URI, e.g. `spotify:track:...`.
    pub uri: String,
    /// Track title.
    pub title: String,
    /// Artist name. Joined with `, ` when a track has several.
    pub artist: String,
    /// Length in milliseconds. Zero when unknown.
    pub duration_ms: i64,
}

/// What the player is doing right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Playback {
    /// The track on the speaker.
    pub track: Track,
    /// How far into it, in milliseconds.
    pub progress_ms: i64,
    /// False when paused.
    pub is_playing: bool,
    /// Name of the device it is coming out of, if the source reports one.
    pub device: Option<String>,
}

impl Playback {
    /// Milliseconds left before this track ends.
    ///
    /// The conductor's pre-push window is measured against this. Clamped at
    /// zero: a source that reports progress past the end of a track should not
    /// produce a negative countdown.
    pub fn remaining_ms(&self) -> i64 {
        (self.track.duration_ms - self.progress_ms).max(0)
    }
}

/// Whatever actually plays music.
#[async_trait]
pub trait Player: Send + Sync + 'static {
    /// What is playing, or `None` when nothing is active.
    async fn playback(&self) -> Result<Option<Playback>>;

    /// Find tracks matching a query.
    async fn search(&self, query: &str) -> Result<Vec<Track>>;

    /// Hand the source exactly one track to play next.
    ///
    /// One at a time, deliberately: Spotify's queue can be added to but not
    /// reordered or emptied, so anything pushed is committed to playing. SESH
    /// keeps the authoritative queue precisely so veto stays possible.
    async fn enqueue(&self, uri: &str) -> Result<()>;

    /// Start this track now, rather than after whatever is playing.
    ///
    /// Distinct from [`enqueue`](Player::enqueue) because Spotify's queue
    /// endpoint appends but never *begins*: with nothing on the speaker,
    /// enqueueing is silent and stays silent. A room whose queue fills up
    /// while nobody is playing anything is the ordinary way an evening
    /// starts, so the conductor needs a way to break that silence.
    async fn play(&self, uri: &str) -> Result<()>;

    /// Abandon the current track and move to whatever is next.
    async fn skip(&self) -> Result<()>;

    /// Move playback onto the room's own device.
    async fn transfer(&self) -> Result<()>;

    /// Stop, without losing the place in the track.
    ///
    /// Used when the room's speaker goes away — see [`audio`](crate::audio).
    /// A source that keeps playing into a sink nobody is listening to burns
    /// through a queue the room never heard.
    async fn pause(&self) -> Result<()>;

    /// Carry on from where [`pause`](Self::pause) stopped.
    async fn resume(&self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(duration_ms: i64, progress_ms: i64) -> Playback {
        Playback {
            track: Track {
                uri: "spotify:track:a".into(),
                title: "A".into(),
                artist: "Someone".into(),
                duration_ms,
            },
            progress_ms,
            is_playing: true,
            device: None,
        }
    }

    #[test]
    fn remaining_counts_down_to_the_end_of_the_track() {
        assert_eq!(playing(210_000, 0).remaining_ms(), 210_000);
        assert_eq!(playing(210_000, 200_000).remaining_ms(), 10_000);
        assert_eq!(playing(210_000, 210_000).remaining_ms(), 0);
    }

    // Spotify has been observed to report progress a little past a track's
    // stated duration. A negative countdown would read as "already over" to
    // the conductor and could push the next track twice.
    #[test]
    fn remaining_never_goes_negative() {
        assert_eq!(playing(210_000, 215_000).remaining_ms(), 0);
    }

    #[test]
    fn remaining_is_the_whole_track_when_the_length_is_unknown() {
        assert_eq!(playing(0, 0).remaining_ms(), 0);
    }
}
