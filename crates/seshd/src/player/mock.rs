//! A [`Player`] that plays nothing and remembers everything.
//!
//! Public rather than `#[cfg(test)]`, matching
//! [`MockPlatform`](crate::launcher::platform::MockPlatform): integration tests
//! live in their own crate and cannot see a test-gated type.
//!
//! This is what makes Phase 4's conductor testable. Every rule about when to
//! push the next track, when a veto skips, and how to behave when the source
//! is unreachable is exercised against this, with a driven clock and no
//! network.

use std::sync::Mutex;

use anyhow::{bail, Result};
use async_trait::async_trait;

use super::{Playback, Player, Track};

/// One call the player received, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `playback` was polled.
    Playback,
    /// `search` was asked for this query.
    Search(String),
    /// This URI was handed to the source.
    Enqueue(String),
    /// This URI was started immediately.
    Play(String),
    /// The current track was abandoned.
    Skip,
    /// Playback was moved to the room's device.
    Transfer,
    /// Playback was stopped without losing its place.
    Pause,
    /// Playback was carried on from where it stopped.
    Resume,
}

#[derive(Debug, Default)]
struct State {
    playback: Option<Playback>,
    results: Vec<Track>,
    calls: Vec<Call>,
    failure: Option<String>,
    write_failure: Option<String>,
}

/// A player that records what it was asked to do.
#[derive(Debug, Default)]
pub struct MockPlayer {
    state: Mutex<State>,
}

impl MockPlayer {
    /// A mock with nothing playing and nothing to find.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set what `playback` will report.
    pub fn set_playback(&self, playback: Option<Playback>) {
        self.lock().playback = playback;
    }

    /// Set what `search` will return.
    pub fn set_results(&self, results: Vec<Track>) {
        self.lock().results = results;
    }

    /// Make every call fail, standing in for an unreachable source.
    ///
    /// The conductor has to keep accepting queue additions while Spotify is
    /// down — "the room still plays media" — and that path needs a way to be
    /// driven from a test.
    pub fn fail_with(&self, message: impl Into<String>) {
        self.lock().failure = Some(message.into());
    }

    /// Fail only the calls that change something.
    ///
    /// A real state, not a contrivance: with no active Connect device Spotify
    /// answers `GET /me/player` with 204 and 404s every write. The source is
    /// reachable, so the conductor must not mark itself offline — it has to
    /// keep trying instead.
    pub fn fail_writes_with(&self, message: impl Into<String>) {
        self.lock().write_failure = Some(message.into());
    }

    /// Stop failing.
    pub fn recover(&self) {
        let mut state = self.lock();
        state.failure = None;
        state.write_failure = None;
    }

    /// Everything asked of this player, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// Every URI started immediately, in order.
    pub fn played(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Play(uri) => Some(uri),
                _ => None,
            })
            .collect()
    }

    /// Every URI handed over, in order.
    pub fn enqueued(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Enqueue(uri) => Some(uri),
                _ => None,
            })
            .collect()
    }

    /// How many times the current track was abandoned.
    pub fn skips(&self) -> usize {
        self.calls().iter().filter(|c| **c == Call::Skip).count()
    }

    /// Forget every recorded call, keeping the scripted state.
    pub fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("mock player mutex poisoned")
    }

    /// Record the call, then fail if this mock is currently broken.
    ///
    /// Recording first is deliberate: a test asserting "the conductor did try
    /// to push a track while Spotify was down" needs the attempt visible.
    fn record(&self, call: Call) -> Result<()> {
        let mut state = self.lock();
        let changes_something = matches!(
            call,
            Call::Enqueue(_) | Call::Play(_) | Call::Skip | Call::Transfer
        );
        state.calls.push(call);
        if let Some(failure) = &state.failure {
            bail!("mock player failure: {failure}");
        }
        if changes_something {
            if let Some(failure) = &state.write_failure {
                bail!("mock player write failure: {failure}");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Player for MockPlayer {
    async fn playback(&self) -> Result<Option<Playback>> {
        self.record(Call::Playback)?;
        Ok(self.lock().playback.clone())
    }

    async fn search(&self, query: &str) -> Result<Vec<Track>> {
        self.record(Call::Search(query.to_string()))?;
        Ok(self.lock().results.clone())
    }

    async fn enqueue(&self, uri: &str) -> Result<()> {
        self.record(Call::Enqueue(uri.to_string()))
    }

    async fn play(&self, uri: &str) -> Result<()> {
        self.record(Call::Play(uri.to_string()))
    }

    async fn skip(&self) -> Result<()> {
        self.record(Call::Skip)
    }

    async fn transfer(&self) -> Result<()> {
        self.record(Call::Transfer)
    }

    async fn pause(&self) -> Result<()> {
        self.record(Call::Pause)
    }

    async fn resume(&self) -> Result<()> {
        self.record(Call::Resume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(uri: &str) -> Track {
        Track {
            uri: uri.into(),
            title: "A".into(),
            artist: "Someone".into(),
            duration_ms: 210_000,
        }
    }

    #[tokio::test]
    async fn a_fresh_mock_is_playing_nothing() {
        let player = MockPlayer::new();
        assert_eq!(player.playback().await.unwrap(), None);
        assert_eq!(player.search("anything").await.unwrap(), vec![]);
    }

    #[tokio::test]
    async fn scripted_playback_is_returned() {
        let player = MockPlayer::new();
        player.set_playback(Some(Playback {
            track: track("spotify:track:a"),
            progress_ms: 1_000,
            is_playing: true,
            device: Some("SESH".into()),
        }));

        let playing = player.playback().await.unwrap().unwrap();
        assert_eq!(playing.track.uri, "spotify:track:a");
        assert_eq!(playing.remaining_ms(), 209_000);
    }

    #[tokio::test]
    async fn every_call_is_recorded_in_order() {
        let player = MockPlayer::new();
        player.transfer().await.unwrap();
        player.enqueue("spotify:track:a").await.unwrap();
        player.skip().await.unwrap();
        let _ = player.search("teenage dirtbag").await;

        assert_eq!(
            player.calls(),
            vec![
                Call::Transfer,
                Call::Enqueue("spotify:track:a".into()),
                Call::Skip,
                Call::Search("teenage dirtbag".into()),
            ]
        );
        assert_eq!(player.enqueued(), vec!["spotify:track:a".to_string()]);
        assert_eq!(player.skips(), 1);
    }

    #[tokio::test]
    async fn a_broken_player_fails_every_call() {
        let player = MockPlayer::new();
        player.fail_with("spotify unreachable");

        assert!(player.playback().await.is_err());
        assert!(player.enqueue("spotify:track:a").await.is_err());
        assert!(player.skip().await.is_err());

        player.recover();
        assert!(player.playback().await.is_ok());
    }

    // The attempt has to be visible even though it failed, so a test can
    // assert the conductor tried rather than silently gave up.
    #[tokio::test]
    async fn a_failed_call_is_still_recorded() {
        let player = MockPlayer::new();
        player.fail_with("down");
        let _ = player.enqueue("spotify:track:a").await;

        assert_eq!(player.enqueued(), vec!["spotify:track:a".to_string()]);
    }

    #[tokio::test]
    async fn calls_can_be_cleared_without_losing_the_script() {
        let player = MockPlayer::new();
        player.set_results(vec![track("spotify:track:a")]);
        let _ = player.search("a").await;
        player.clear_calls();

        assert!(player.calls().is_empty());
        assert_eq!(player.search("a").await.unwrap().len(), 1);
    }
}
