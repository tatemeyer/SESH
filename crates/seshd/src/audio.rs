//! Whether the room's speaker is there, and what the log says when it is not.
//!
//! The Victrola is a record player that is also a Bluetooth speaker, and those
//! two things are the same input. Put a record on and Bluetooth stops being the
//! active input, so the A2DP link drops and the sink disappears from PipeWire.
//!
//! That is not a fault to be recovered from. **The speaker disconnecting is the
//! vinyl handoff signal** — the room asking to play a record instead — and it
//! arrives for free as a side effect of noticing the sink went away. This arc
//! only records it and stops pushing music at a speaker that is not listening;
//! making the room *react* is a later arc reading events that will by then
//! already be in the log.
//!
//! Plan: `docs/superpowers/plans/2026-08-17-arc2-phones-and-queue.md`, Phase 6.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::event::{kind, NewEvent};
use crate::player::Player;
use crate::room::Room;

/// Sink names beginning with this belong to a Bluetooth device.
///
/// PipeWire names A2DP sinks `bluez_output.<MAC>.<profile>`. Matching the
/// prefix rather than the full name means `seshd` needs no configuration: the
/// MAC is not known until the speaker is paired, and the room has one speaker.
pub const BLUETOOTH_PREFIX: &str = "bluez_output.";

/// How often to look. A record going on is a human-scale event; polling faster
/// buys nothing and wakes the CPU on an idle box all evening.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(5);

/// The audio sinks the box can play to.
pub trait Sinks: Send + Sync {
    /// Names of every sink currently available.
    fn names(&self) -> Result<Vec<String>>;
}

/// Real sinks, read from PipeWire through `pactl`.
pub struct PactlSinks;

impl Sinks for PactlSinks {
    fn names(&self) -> Result<Vec<String>> {
        let output = Command::new("pactl")
            .args(["list", "short", "sinks"])
            .output()
            .context("running pactl")?;

        interpret_pactl(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
    }
}

/// Turn a finished `pactl` into sink names, or an error.
///
/// Split out from the process call for one reason: **the status check is the
/// whole defect, and inside `PactlSinks` it cannot be reached by a test.**
///
/// `pactl` that fails — PipeWire not up yet, the socket gone, the session not
/// ours — writes nothing to stdout, and an empty stdout parses to an empty sink
/// list. Reporting that as success makes "the query failed" indistinguishable
/// from "there are no sinks", and the watcher turns the second into
/// `audio.sink_lost`: the room announcing the speaker left because it could not
/// ask. The same distinction Arc 3's fusion is built on — no answer is not a
/// negative answer.
pub fn interpret_pactl(success: bool, stdout: &str, stderr: &str) -> Result<Vec<String>> {
    if !success {
        bail!("pactl failed: {}", stderr.trim());
    }
    Ok(parse_sinks(stdout))
}

/// Pull sink names out of `pactl list short sinks`.
///
/// Tab-separated, name in the second column. Split out so the parsing is
/// testable without a running PipeWire.
pub fn parse_sinks(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .map(str::to_string)
        .collect()
}

/// Sinks a test hands over directly.
pub struct MockSinks {
    names: std::sync::Mutex<Vec<String>>,
    failure: std::sync::Mutex<Option<String>>,
}

impl MockSinks {
    /// Start with these sinks present.
    pub fn new(names: &[&str]) -> Self {
        Self {
            names: std::sync::Mutex::new(names.iter().map(|s| s.to_string()).collect()),
            failure: std::sync::Mutex::new(None),
        }
    }

    /// Make the next look fail, the way a `pactl` that cannot reach PipeWire
    /// does.
    ///
    /// This exists because the double could not previously express it, and a
    /// double that can only give the documented answer cannot catch a caller
    /// that mishandles any other one.
    pub fn fail_with(&self, message: impl Into<String>) {
        *self.failure.lock().expect("mock sinks poisoned") = Some(message.into());
    }

    /// Stop failing.
    pub fn recover(&self) {
        *self.failure.lock().expect("mock sinks poisoned") = None;
    }

    /// Replace what the next look will find.
    pub fn set(&self, names: &[&str]) {
        *self.names.lock().expect("mock sinks poisoned") =
            names.iter().map(|s| s.to_string()).collect();
    }
}

impl Sinks for MockSinks {
    fn names(&self) -> Result<Vec<String>> {
        if let Some(failure) = self.failure.lock().expect("mock sinks poisoned").as_ref() {
            bail!("mock sinks failure: {failure}");
        }
        Ok(self.names.lock().expect("mock sinks poisoned").clone())
    }
}

/// What changed since the last look, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The speaker appeared. Carries its sink name.
    Found(String),
    /// The speaker went away. Carries the sink name it had.
    Lost(String),
}

/// Turns a sequence of observations into transitions.
///
/// Pure: no clock, no I/O, no `Room`. Transitions only, like [`Presence`] — the
/// log gains a row when the speaker comes or goes, not one every five seconds
/// all evening.
///
/// [`Presence`]: crate::presence::Presence
pub struct SinkWatch {
    prefix: String,
    /// `None` until the first look. Seeding rather than reporting means a
    /// restart while the speaker is connected does not invent a `sink_found`
    /// for a speaker that never went anywhere.
    speaker: Option<Option<String>>,
}

impl SinkWatch {
    /// Watch for sinks whose name starts with `prefix`.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            speaker: None,
        }
    }

    /// Feed one observation and get back what changed.
    pub fn observe(&mut self, names: &[String]) -> Option<Change> {
        let found = names
            .iter()
            .find(|name| name.starts_with(&self.prefix))
            .cloned();

        // First look: seed and say nothing. There is no transition to report
        // when there was nothing to transition from.
        let previous = self.speaker.replace(found.clone())?;

        match (previous, found) {
            (None, Some(name)) => Some(Change::Found(name)),
            (Some(name), None) => Some(Change::Lost(name)),
            // Still there, still gone, or swapped for a different Bluetooth
            // sink — the last is not a transition of the thing being watched.
            _ => None,
        }
    }
}

/// Record sink changes forever, pausing the music when the speaker goes away.
///
/// Shaped like `presence::sweep_loop`: the timing lives next to the logic it
/// drives so `main` stays a wiring file.
pub async fn watch_loop(
    sinks: Arc<dyn Sinks>,
    room: Arc<Room>,
    player: Option<Arc<dyn Player>>,
    prefix: String,
) {
    let mut watch = SinkWatch::new(prefix);
    let mut ticker = tokio::time::interval(WATCH_INTERVAL);
    loop {
        ticker.tick().await;

        let names = match sinks.names() {
            Ok(names) => names,
            Err(error) => {
                tracing::warn!(%error, "could not list audio sinks");
                continue;
            }
        };

        let Some(change) = watch.observe(&names) else {
            continue;
        };

        if let Err(error) = act_on(&change, &room, player.as_ref()).await {
            tracing::error!(%error, ?change, "recording a sink change failed");
        }
    }
}

/// Record one change and tell the source about it.
async fn act_on(change: &Change, room: &Room, player: Option<&Arc<dyn Player>>) -> Result<()> {
    let (event_kind, sink) = match change {
        Change::Found(sink) => (kind::AUDIO_SINK_FOUND, sink),
        Change::Lost(sink) => (kind::AUDIO_SINK_LOST, sink),
    };

    // The log first, and unconditionally. A box with no Spotify credentials
    // still records that its speaker came and went; that is the vinyl signal a
    // later arc will read, and it does not depend on there being a source.
    room.record(NewEvent::new(event_kind).subject(sink.clone()))?;

    let Some(player) = player else {
        return Ok(());
    };

    // Telling the source is best-effort, and the failure is *not* returned.
    // The row is already written, which is the part that matters; and the
    // ordinary case for a room where nobody has started any music is that
    // Spotify has no active device and answers 404 to both of these. Returning
    // an error here put an ERROR in the log on every speaker reconnect, which
    // teaches whoever reads it to ignore the level.
    let outcome = match change {
        Change::Lost(_) => {
            tracing::info!(%sink, "the speaker went away; pausing");
            player.pause().await
        }
        Change::Found(_) => {
            tracing::info!(%sink, "the speaker is back; resuming");
            player.resume().await
        }
    };
    if let Err(error) = outcome {
        tracing::warn!(%error, %sink, "recorded the change, but the source did not take it");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDMI: &str = "alsa_output.platform-107c706400.hdmi.hdmi-stereo";
    const VICTROLA: &str = "bluez_output.E9_FE_A0_81_8C_E0.1";

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The bug this file had until 2026-08-21, found by auditing what the
    /// doubles could not express.
    ///
    /// `PactlSinks::names` ignored the exit status. A `pactl` that fails —
    /// PipeWire not up yet, socket gone, session not ours — writes nothing to
    /// stdout, empty stdout parses to an empty sink list, and `Ok(vec![])` is
    /// indistinguishable from "there are no sinks". The watcher turned that
    /// into `audio.sink_lost`: **the room announcing the speaker left because
    /// it could not ask.**
    ///
    /// The loop was always written for this — it warns and skips on `Err` — so
    /// the whole defect was the one `Ok` that should have been an error. And
    /// `MockSinks` could not fail, so nothing could have caught it.
    ///
    /// Same distinction Arc 3's fusion is built on: no answer is not the same
    /// as a negative answer.
    #[tokio::test(start_paused = true)]
    async fn a_failed_look_is_not_a_departure() {
        use crate::room::Room;
        use crate::store::Store;

        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let sinks = std::sync::Arc::new(MockSinks::new(&[HDMI, VICTROLA]));
        let watcher = tokio::spawn(watch_loop(
            sinks.clone(),
            room.clone(),
            None,
            "bluez_output.".to_string(),
        ));

        // Let it seed on a room where the speaker is present.
        tokio::time::advance(WATCH_INTERVAL * 2).await;
        tokio::task::yield_now().await;

        // Now the query itself breaks. The speaker has not moved.
        sinks.fail_with("Connection refused");
        for _ in 0..5 {
            tokio::time::advance(WATCH_INTERVAL).await;
            tokio::task::yield_now().await;
        }
        watcher.abort();

        let lost: Vec<_> = room
            .events_since(0, -1)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind::AUDIO_SINK_LOST)
            .collect();
        assert!(
            lost.is_empty(),
            "a look that failed must not be reported as the speaker leaving, got {lost:?}"
        );
    }

    /// The defect itself, at the only seam where it is reachable.
    ///
    /// The first version of the test above used `MockSinks` and so exercised
    /// the *loop's* handling of `Err` — which was already correct before the
    /// fix. It would have passed against the broken code. This one fails
    /// against it, because the broken code answered `Ok(vec![])` here.
    #[test]
    fn a_pactl_that_failed_is_an_error_not_an_empty_room() {
        // Exactly the shape the bug produced: non-zero exit, nothing on stdout.
        let silent_failure = interpret_pactl(false, "", "");
        assert!(
            silent_failure.is_err(),
            "a failed pactl wrote no sinks; that is not the same as no sinks existing"
        );

        assert!(interpret_pactl(false, "", "Connection refused").is_err());

        // And success still parses.
        let stdout = "66\tbluez_output.E9_FE_A0_81_8C_E0.1\tPipeWire\ts16le\tRUNNING\n";
        assert_eq!(
            interpret_pactl(true, stdout, "").unwrap(),
            names(&[VICTROLA])
        );

        // A genuinely empty room is still empty, not an error.
        assert!(interpret_pactl(true, "", "").unwrap().is_empty());
    }

    /// And the double must actually be able to fail, or the test above is
    /// theatre. Guards the capability itself.
    #[test]
    fn the_sink_double_can_fail_the_way_the_real_one_does() {
        let sinks = MockSinks::new(&[HDMI]);
        assert!(sinks.names().is_ok());

        sinks.fail_with("Connection refused");
        assert!(
            sinks.names().is_err(),
            "a double that cannot fail cannot catch a caller that mishandles failure"
        );

        sinks.recover();
        assert_eq!(sinks.names().unwrap(), names(&[HDMI]));
    }

    #[test]
    fn sinks_are_read_out_of_pactls_second_column() {
        let stdout = "65\talsa_output.platform-107c706400.hdmi.hdmi-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n\
                      66\tbluez_output.E9_FE_A0_81_8C_E0.1\tPipeWire\ts16le 2ch 44100Hz\tRUNNING\n";
        assert_eq!(parse_sinks(stdout), names(&[HDMI, VICTROLA]));
    }

    #[test]
    fn no_sinks_at_all_is_not_an_error() {
        assert!(parse_sinks("").is_empty());
        assert!(parse_sinks("\n\n").is_empty());
    }

    // A box with only HDMI must not look like a box whose speaker just left.
    #[test]
    fn the_first_look_seeds_and_reports_nothing() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        assert_eq!(watch.observe(&names(&[HDMI])), None);
    }

    #[test]
    fn a_restart_with_the_speaker_connected_reports_nothing() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        assert_eq!(watch.observe(&names(&[HDMI, VICTROLA])), None);
    }

    #[test]
    fn the_speaker_appearing_is_found() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        watch.observe(&names(&[HDMI]));
        assert_eq!(
            watch.observe(&names(&[HDMI, VICTROLA])),
            Some(Change::Found(VICTROLA.to_string()))
        );
    }

    // Putting a record on: the Victrola switches to phono and the A2DP link
    // drops. This is the vinyl handoff signal, not a fault.
    #[test]
    fn the_speaker_going_away_is_lost_and_names_it() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        watch.observe(&names(&[HDMI, VICTROLA]));
        assert_eq!(
            watch.observe(&names(&[HDMI])),
            Some(Change::Lost(VICTROLA.to_string()))
        );
    }

    #[test]
    fn nothing_is_reported_while_the_speaker_stays_put() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        watch.observe(&names(&[HDMI, VICTROLA]));
        assert_eq!(watch.observe(&names(&[HDMI, VICTROLA])), None);
        assert_eq!(watch.observe(&names(&[HDMI, VICTROLA])), None);
    }

    #[test]
    fn a_speaker_that_comes_back_reports_found_again() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        watch.observe(&names(&[HDMI, VICTROLA]));
        assert_eq!(
            watch.observe(&names(&[HDMI])),
            Some(Change::Lost(VICTROLA.into()))
        );
        assert_eq!(
            watch.observe(&names(&[HDMI, VICTROLA])),
            Some(Change::Found(VICTROLA.into()))
        );
    }

    // HDMI is not the speaker. Losing it would be a different problem.
    #[test]
    fn only_bluetooth_sinks_count_as_the_speaker() {
        let mut watch = SinkWatch::new(BLUETOOTH_PREFIX);
        watch.observe(&names(&[HDMI]));
        assert_eq!(watch.observe(&names(&[])), None);
    }

    // Against the real PipeWire on this box. Ignored by default for the same
    // reason as the live Spotify probe: it needs a running session, which a
    // build machine does not have.
    #[test]
    #[ignore = "needs a running PipeWire; run on the Pi with --ignored"]
    fn pactl_finds_at_least_one_real_sink() {
        let found = PactlSinks.names().unwrap();
        assert!(!found.is_empty(), "a Pi wired to a TV has at least HDMI");
        assert!(
            found.iter().any(|n| n.starts_with("alsa_output.")),
            "expected an ALSA sink, got {found:?}"
        );
    }

    #[tokio::test]
    async fn losing_the_speaker_records_it_and_pauses_the_source() {
        use crate::player::mock::{Call, MockPlayer};
        use crate::store::Store;

        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let player = Arc::new(MockPlayer::new());
        let as_player: Arc<dyn Player> = player.clone();

        act_on(&Change::Lost(VICTROLA.into()), &room, Some(&as_player))
            .await
            .unwrap();

        let log = room.events_since(0, -1).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].kind, kind::AUDIO_SINK_LOST);
        assert_eq!(log[0].subject.as_deref(), Some(VICTROLA));
        assert!(player.calls().contains(&Call::Pause), "the music must stop");
    }

    #[tokio::test]
    async fn finding_the_speaker_records_it_and_resumes() {
        use crate::player::mock::{Call, MockPlayer};
        use crate::store::Store;

        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let player = Arc::new(MockPlayer::new());
        let as_player: Arc<dyn Player> = player.clone();

        act_on(&Change::Found(VICTROLA.into()), &room, Some(&as_player))
            .await
            .unwrap();

        let log = room.events_since(0, -1).unwrap();
        assert_eq!(log[0].kind, kind::AUDIO_SINK_FOUND);
        assert!(player.calls().contains(&Call::Resume));
    }

    // The common case in a quiet room: Spotify has no active device, so pause
    // and resume both 404. The log must still gain its row, and the failure
    // must not read as though recording failed.
    #[tokio::test]
    async fn a_source_that_refuses_still_leaves_the_row_behind() {
        use crate::player::mock::MockPlayer;
        use crate::store::Store;

        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let player = Arc::new(MockPlayer::new());
        player.fail_with("no active device");
        let as_player: Arc<dyn Player> = player.clone();

        act_on(&Change::Lost(VICTROLA.into()), &room, Some(&as_player))
            .await
            .expect("a source that refuses is not a failure to record");

        let log = room.events_since(0, -1).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].kind, kind::AUDIO_SINK_LOST);
    }

    // A box with no Spotify credentials still keeps the log honest.
    #[tokio::test]
    async fn a_room_with_no_source_still_records_the_change() {
        use crate::store::Store;

        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        act_on(&Change::Lost(VICTROLA.into()), &room, None)
            .await
            .unwrap();

        let log = room.events_since(0, -1).unwrap();
        assert_eq!(log[0].kind, kind::AUDIO_SINK_LOST);
    }
}
