//! The conductor, against a mock source with no clock and no network.
//!
//! `tick()` does one pass and returns the next interval, so every rule here is
//! driven by calling it rather than by waiting. A test that slept would be
//! slow and flaky; one that advances a fake clock would still be testing
//! tokio's timer rather than the conductor's decisions.

use std::sync::Arc;

use seshd::conductor::{Conductor, Status, POLL_IDLE, POLL_OFFLINE, POLL_PLAYING, PREPUSH_MS};
use seshd::event::{kind, Event, NewEvent};
use seshd::player::mock::MockPlayer;
use seshd::player::{Playback, Track};
use seshd::room::Room;
use seshd::store::Store;

struct Fixture {
    room: Arc<Room>,
    player: Arc<MockPlayer>,
    conductor: Arc<Conductor>,
    status: Arc<Status>,
}

fn fixture() -> Fixture {
    let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
    let player = Arc::new(MockPlayer::new());
    let status = Arc::new(Status::new());
    let conductor = Conductor::new(room.clone(), player.clone(), status.clone());
    Fixture {
        room,
        player,
        conductor,
        status,
    }
}

impl Fixture {
    /// Queue a track the way `POST /api/music/queue` would, returning its id.
    fn queue(&self, uri: &str, title: &str, who: &str) -> i64 {
        self.room
            .record(
                NewEvent::new(kind::MUSIC_QUEUED)
                    .actor(who)
                    .subject(uri)
                    .payload(serde_json::json!({
                        "title": title, "artist": "Someone", "duration_ms": 210_000
                    })),
            )
            .unwrap()
            .id
    }

    fn arrive(&self, who: &str) {
        self.room
            .record(NewEvent::new(kind::PRESENCE_ARRIVED).actor(who))
            .unwrap();
    }

    fn veto(&self, entry: i64, uri: &str, who: &str) {
        self.room
            .record(
                NewEvent::new(kind::MUSIC_VETOED)
                    .actor(who)
                    .subject(uri)
                    .payload(serde_json::json!({ "entry": entry })),
            )
            .unwrap();
    }

    /// Put a track on the mock speaker.
    fn speaker(&self, uri: &str, progress_ms: i64) {
        self.player.set_playback(Some(Playback {
            track: Track {
                uri: uri.into(),
                title: "A".into(),
                artist: "Someone".into(),
                duration_ms: 210_000,
            },
            progress_ms,
            is_playing: true,
            device: Some("SESH".into()),
        }));
    }

    fn silence(&self) {
        self.player.set_playback(None);
    }

    /// What the log believes is playing, detached from the projection.
    fn playing(&self) -> Option<seshd::projections::queue::Entry> {
        self.room.queue().now_playing().cloned()
    }

    fn events(&self, of_kind: &str) -> Vec<Event> {
        self.room
            .events_since(0, -1)
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == of_kind)
            .collect()
    }
}

// ---------------------------------------------------------------- 4.1 the loop

#[tokio::test]
async fn a_quiet_room_with_an_empty_queue_is_left_alone() {
    let f = fixture();
    let wait = f.conductor.tick().await;

    assert_eq!(f.player.played(), Vec::<String>::new());
    assert_eq!(f.player.enqueued(), Vec::<String>::new());
    assert!(f.events(kind::MUSIC_STARTED).is_empty());
    assert_eq!(wait, POLL_IDLE, "nothing playing polls slowly");
}

// The cold start. `enqueue` appends to a queue nothing is draining, so a room
// whose queue fills up in silence would stay silent.
#[tokio::test]
async fn a_queued_track_is_started_when_nothing_is_playing() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");

    f.conductor.tick().await;

    assert_eq!(f.player.played(), vec!["spotify:track:a".to_string()]);
    assert!(
        f.player.enqueued().is_empty(),
        "a cold start must not go out as an append"
    );
}

// D8: nothing is recorded as started until the speaker confirms it.
#[tokio::test]
async fn a_track_is_recorded_started_only_once_the_speaker_confirms() {
    let f = fixture();
    let entry = f.queue("spotify:track:a", "A", "sam");

    f.conductor.tick().await;
    assert!(
        f.events(kind::MUSIC_STARTED).is_empty(),
        "the source has not said it is playing yet"
    );

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;

    let started = f.events(kind::MUSIC_STARTED);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].payload["entry"], entry);
    assert_eq!(f.room.queue().now_playing().unwrap().entry, entry);
    assert!(f.room.queue().pending().is_empty());
}

#[tokio::test]
async fn the_successor_is_handed_over_before_the_current_track_ends() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    f.queue("spotify:track:b", "B", "marcus");

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;
    assert!(f.player.enqueued().is_empty(), "far too early to push");

    // Inside the window.
    f.speaker("spotify:track:a", 210_000 - PREPUSH_MS + 1);
    let wait = f.conductor.tick().await;

    assert_eq!(f.player.enqueued(), vec!["spotify:track:b".to_string()]);
    assert_eq!(wait, POLL_PLAYING, "a playing room polls quickly");
}

#[tokio::test]
async fn the_successor_is_handed_over_only_once() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    f.queue("spotify:track:b", "B", "marcus");
    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;

    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;
    f.speaker("spotify:track:a", 209_500);
    f.conductor.tick().await;

    assert_eq!(
        f.player.enqueued(),
        vec!["spotify:track:b".to_string()],
        "pushing twice would put the track in Spotify's queue twice"
    );
}

#[tokio::test]
async fn a_track_that_ends_is_recorded_as_finished() {
    let f = fixture();
    let entry = f.queue("spotify:track:a", "A", "sam");
    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;

    f.silence();
    f.conductor.tick().await;

    let skipped = f.events(kind::MUSIC_SKIPPED);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].payload["entry"], entry);
    assert_eq!(skipped[0].payload["why"], "finished");
    assert_eq!(f.room.queue().now_playing(), None);
}

// A pause is not an ending. Recording one as finished would drop the track off
// the TV card the moment somebody answered the door.
#[tokio::test]
async fn pausing_is_not_the_same_as_finishing() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    f.speaker("spotify:track:a", 30_000);
    f.conductor.tick().await;

    f.player.set_playback(Some(Playback {
        track: Track {
            uri: "spotify:track:a".into(),
            title: "A".into(),
            artist: "Someone".into(),
            duration_ms: 210_000,
        },
        progress_ms: 30_000,
        is_playing: false,
        device: Some("SESH".into()),
    }));
    let wait = f.conductor.tick().await;

    assert!(f.events(kind::MUSIC_SKIPPED).is_empty());
    assert!(f.room.queue().now_playing().is_some());
    assert_eq!(wait, POLL_IDLE, "a paused room does not need 3s polling");
}

// D8 again: the log records what the house did, including things it did
// somewhere else entirely.
#[tokio::test]
async fn a_track_nobody_queued_here_is_still_recorded() {
    let f = fixture();
    f.speaker("spotify:track:elsewhere", 5_000);

    f.conductor.tick().await;

    let playing = f.playing().expect("must reflect reality");
    assert_eq!(playing.uri, "spotify:track:elsewhere");
    assert_eq!(playing.added_by, None, "nobody here queued it");
}

#[tokio::test]
async fn an_empty_queue_is_never_filled_with_suggestions() {
    let f = fixture();
    let entry = f.queue("spotify:track:a", "A", "sam");
    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;
    assert_eq!(f.room.queue().now_playing().unwrap().entry, entry);

    // Inside the pre-push window with nothing to push.
    f.conductor.tick().await;

    assert!(f.player.enqueued().is_empty());
    assert!(f.player.played().is_empty());
}

// ------------------------------------------------------------------- vetoes

#[tokio::test]
async fn a_vetoed_playing_track_is_skipped() {
    let f = fixture();
    f.arrive("sam");
    f.arrive("marcus");
    let entry = f.queue("spotify:track:a", "A", "sam");
    f.speaker("spotify:track:a", 10_000);
    f.conductor.tick().await;

    f.veto(entry, "spotify:track:a", "sam");
    f.veto(entry, "spotify:track:a", "marcus");
    f.conductor.tick().await;

    assert_eq!(f.player.skips(), 1);
    let vetoed = f
        .events(kind::MUSIC_SKIPPED)
        .into_iter()
        .find(|e| e.payload["why"] == "vetoed")
        .expect("a vetoed skip");
    assert_eq!(vetoed.payload["entry"], entry);
    assert_eq!(f.room.queue().now_playing(), None);
}

#[tokio::test]
async fn one_vote_short_of_the_threshold_changes_nothing() {
    let f = fixture();
    f.arrive("sam");
    f.arrive("marcus");
    let entry = f.queue("spotify:track:a", "A", "sam");
    f.speaker("spotify:track:a", 10_000);
    f.conductor.tick().await;

    f.veto(entry, "spotify:track:a", "sam");
    f.conductor.tick().await;

    assert_eq!(f.player.skips(), 0);
    assert!(f.room.queue().now_playing().is_some());
}

// A waiting track is dropped from SESH's own queue. No call to the source:
// it was never handed over, so there is nothing there to cancel.
#[tokio::test]
async fn a_vetoed_waiting_track_is_dropped_without_touching_the_source() {
    let f = fixture();
    f.arrive("sam");
    f.arrive("marcus");
    f.queue("spotify:track:a", "A", "sam");
    let doomed = f.queue("spotify:track:b", "B", "marcus");
    f.speaker("spotify:track:a", 10_000);
    f.conductor.tick().await;

    f.veto(doomed, "spotify:track:b", "sam");
    f.veto(doomed, "spotify:track:b", "marcus");
    f.conductor.tick().await;

    assert_eq!(f.player.skips(), 0);
    assert!(f.room.queue().pending().is_empty());
    assert!(f.player.enqueued().is_empty());
}

// D7, the honest cost of the pre-push window: the track is already committed
// in Spotify's queue, so it starts, and is killed a beat later.
#[tokio::test]
async fn a_track_vetoed_after_being_pushed_starts_and_is_skipped_at_once() {
    let f = fixture();
    f.arrive("sam");
    f.arrive("marcus");
    f.queue("spotify:track:a", "A", "sam");
    let doomed = f.queue("spotify:track:b", "B", "marcus");

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;
    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;
    assert_eq!(f.player.enqueued(), vec!["spotify:track:b".to_string()]);

    // Too late. It is committed, so it must not vanish from the queue.
    f.veto(doomed, "spotify:track:b", "sam");
    f.veto(doomed, "spotify:track:b", "marcus");
    f.conductor.tick().await;
    assert_eq!(
        f.room.queue().pending().len(),
        1,
        "a committed track must not be dropped; Spotify is going to play it"
    );
    assert_eq!(f.player.skips(), 0, "nothing to skip while A is still on");

    // It starts, exactly as Spotify promised, and dies immediately.
    f.speaker("spotify:track:b", 500);
    f.conductor.tick().await;

    assert_eq!(f.player.skips(), 1);
    let reasons: Vec<String> = f
        .events(kind::MUSIC_SKIPPED)
        .iter()
        .map(|e| e.payload["why"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        reasons.contains(&"vetoed".to_string()),
        "the log must say it was vetoed, not that it finished: {reasons:?}"
    );
    assert_eq!(f.room.queue().now_playing(), None);
}

// D1 through the conductor: the same song queued twice is two entries, and
// the second copy starting looks identical to the first except for progress.
#[tokio::test]
async fn the_same_song_queued_twice_starts_twice() {
    let f = fixture();
    let first = f.queue("spotify:track:a", "A", "sam");
    let second = f.queue("spotify:track:a", "A", "marcus");
    assert_ne!(first, second);

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;
    assert_eq!(f.room.queue().now_playing().unwrap().entry, first);

    // Pre-push the second copy, then let it begin.
    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;
    f.speaker("spotify:track:a", 200);
    f.conductor.tick().await;

    assert_eq!(
        f.room.queue().now_playing().unwrap().entry,
        second,
        "the second copy must become the playing entry"
    );
    assert!(f.room.queue().pending().is_empty());
}

// The other half of that pair: dragging the scrubber back is not a new track.
#[tokio::test]
async fn seeking_backwards_does_not_start_the_track_again() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    f.speaker("spotify:track:a", 120_000);
    f.conductor.tick().await;

    f.speaker("spotify:track:a", 3_000);
    f.conductor.tick().await;

    assert_eq!(
        f.events(kind::MUSIC_STARTED).len(),
        1,
        "a seek is not a second play"
    );
}

// -------------------------------------------------- 4.2 startup reconciliation

// Not Arc 1's close-the-dangling-row: librespot outlives seshd, so the log is
// what is wrong here, not the speaker.
#[tokio::test]
async fn startup_believes_the_speaker_over_the_log() {
    let f = fixture();
    let stale = f.queue("spotify:track:a", "A", "sam");
    f.room
        .record(
            NewEvent::new(kind::MUSIC_STARTED)
                .subject("spotify:track:a")
                .payload(serde_json::json!({ "entry": stale })),
        )
        .unwrap();
    assert_eq!(f.room.queue().now_playing().unwrap().entry, stale);

    // Something else entirely is on the speaker.
    f.speaker("spotify:track:z", 40_000);
    f.conductor.tick().await;

    let playing = f.playing().unwrap();
    assert_eq!(playing.uri, "spotify:track:z");
    let closed = f
        .events(kind::MUSIC_SKIPPED)
        .into_iter()
        .find(|e| e.payload["entry"] == stale)
        .expect("the stale track must be closed out");
    assert_eq!(closed.payload["why"], "finished");
}

#[tokio::test]
async fn startup_closes_a_track_the_speaker_is_no_longer_playing() {
    let f = fixture();
    let stale = f.queue("spotify:track:a", "A", "sam");
    f.room
        .record(
            NewEvent::new(kind::MUSIC_STARTED)
                .subject("spotify:track:a")
                .payload(serde_json::json!({ "entry": stale })),
        )
        .unwrap();

    f.silence();
    f.conductor.tick().await;

    assert_eq!(f.room.queue().now_playing(), None);
    assert_eq!(f.events(kind::MUSIC_SKIPPED)[0].payload["why"], "finished");
}

#[tokio::test]
async fn startup_leaves_a_log_that_already_agrees_alone() {
    let f = fixture();
    let entry = f.queue("spotify:track:a", "A", "sam");
    f.room
        .record(
            NewEvent::new(kind::MUSIC_STARTED)
                .subject("spotify:track:a")
                .payload(serde_json::json!({ "entry": entry })),
        )
        .unwrap();

    f.speaker("spotify:track:a", 40_000);
    f.conductor.tick().await;

    assert!(
        f.events(kind::MUSIC_SKIPPED).is_empty(),
        "nothing to correct"
    );
    assert_eq!(f.events(kind::MUSIC_STARTED).len(), 1, "no duplicate start");
}

// ----------------------------------------------------- 4.3 degrade honestly

#[tokio::test]
async fn an_unreachable_source_backs_off_and_says_so() {
    let f = fixture();
    f.player.fail_with("spotify unreachable");

    let wait = f.conductor.tick().await;

    assert_eq!(wait, POLL_OFFLINE);
    assert_eq!(f.status.label(), "offline");
}

#[tokio::test]
async fn the_queue_keeps_working_while_the_source_is_down() {
    let f = fixture();
    f.player.fail_with("spotify unreachable");
    f.conductor.tick().await;

    // The room can still decide what it wants to hear.
    let entry = f.queue("spotify:track:a", "A", "sam");
    f.conductor.tick().await;

    assert_eq!(f.room.queue().pending().len(), 1);
    assert_eq!(f.room.queue().pending()[0].entry, entry);
    assert_eq!(f.status.label(), "offline");
}

#[tokio::test]
async fn the_source_coming_back_is_noticed() {
    let f = fixture();
    f.player.fail_with("down");
    f.conductor.tick().await;
    assert_eq!(f.status.label(), "offline");

    f.player.recover();
    f.queue("spotify:track:a", "A", "sam");
    let wait = f.conductor.tick().await;

    assert_eq!(f.status.label(), "ok");
    assert_eq!(wait, POLL_IDLE);
    assert_eq!(
        f.player.played(),
        vec!["spotify:track:a".to_string()],
        "the backlog should start playing once the source answers"
    );
}

// A failed hand-over must not be remembered as a success, or the track would
// sit in SESH's queue believing Spotify has it and never be pushed again.
#[tokio::test]
async fn a_failed_hand_over_is_retried_on_the_next_pass() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");

    // Reachable enough to poll, but every write 404s — the no-active-device
    // state confirmed against real Spotify in task 3.4.
    f.player.set_playback(None);
    f.player.fail_writes_with("no active device");
    f.conductor.tick().await;

    assert_eq!(
        f.player.played(),
        vec!["spotify:track:a".to_string()],
        "it should have tried"
    );
    assert_eq!(
        f.status.label(),
        "ok",
        "a reachable source that refuses a write is not an offline source"
    );

    f.player.recover();
    f.conductor.tick().await;

    assert_eq!(
        f.player.played(),
        vec!["spotify:track:a".to_string(), "spotify:track:a".to_string()],
        "a hand-over that failed must not be remembered as done"
    );
}

// ------------------------------------------- gaps found by mutation testing
//
// Every test below exists because `cargo mutants` broke the conductor in a
// specific way and the suite above stayed green. None of them are hypothetical
// — each names a way the room misbehaves.

// Spotify's reported progress jitters between polls. Without the REWIND_MS
// threshold, any backwards wobble during the pre-push window reads as "the
// same song started again" and writes a spurious `music.started` into an
// append-only log, every few seconds, for the length of the track.
#[tokio::test]
async fn a_small_backwards_jitter_is_not_a_replay() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    f.queue("spotify:track:b", "B", "marcus");

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;
    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;
    assert_eq!(f.player.enqueued().len(), 1, "B should be committed by now");

    // Backwards, but only slightly: this is jitter, not a new track.
    f.speaker("spotify:track:a", 208_500);
    f.conductor.tick().await;

    assert_eq!(
        f.events(kind::MUSIC_STARTED).len(),
        1,
        "a half-second wobble is not a second play"
    );
}

// The boundary itself. A jump of exactly REWIND_MS is still not a replay —
// the threshold is the smallest jump that counts, not the largest that does
// not.
#[tokio::test]
async fn a_backwards_jump_of_exactly_the_threshold_is_not_a_replay() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    f.queue("spotify:track:b", "B", "marcus");

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;
    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;

    f.speaker("spotify:track:a", 209_000 - 10_000);
    f.conductor.tick().await;

    assert_eq!(f.events(kind::MUSIC_STARTED).len(), 1);
}

// A committed track is not a licence to mislabel whatever plays next. If
// somebody starts something else in the Spotify app while B is sitting in
// Spotify's queue, the log must record what is actually on the speaker — and
// B must stay in SESH's queue, because Spotify is still going to play it.
#[tokio::test]
async fn a_pushed_track_does_not_claim_a_different_song_that_starts() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");
    let committed = f.queue("spotify:track:b", "B", "marcus");

    f.speaker("spotify:track:a", 0);
    f.conductor.tick().await;
    f.speaker("spotify:track:a", 209_000);
    f.conductor.tick().await;
    assert_eq!(f.player.enqueued(), vec!["spotify:track:b".to_string()]);

    // Someone picks something else entirely in the Spotify app.
    f.speaker("spotify:track:elsewhere", 1_000);
    f.conductor.tick().await;

    let playing = f.playing().expect("must reflect reality");
    assert_eq!(playing.uri, "spotify:track:elsewhere");
    assert_eq!(
        playing.added_by, None,
        "nobody in the room queued what is playing"
    );
    assert_eq!(
        f.room.queue().pending().len(),
        1,
        "B is still committed to Spotify and must stay in the queue"
    );
    assert_eq!(f.room.queue().pending()[0].entry, committed);
}

// Once a handed-over track is playing it is no longer pending, and forgetting
// to say so leaves the conductor permanently believing something is committed.
// The visible symptom is that every later backwards seek becomes a replay.
#[tokio::test]
async fn a_track_that_starts_stops_counting_as_committed() {
    let f = fixture();
    f.queue("spotify:track:a", "A", "sam");

    // Cold start: this is the path that marks A as handed over.
    f.conductor.tick().await;
    assert_eq!(f.player.played(), vec!["spotify:track:a".to_string()]);

    f.speaker("spotify:track:a", 120_000);
    f.conductor.tick().await;
    assert_eq!(f.events(kind::MUSIC_STARTED).len(), 1);

    // A long way backwards. With A no longer committed this is a seek.
    f.speaker("spotify:track:a", 3_000);
    f.conductor.tick().await;

    assert_eq!(
        f.events(kind::MUSIC_STARTED).len(),
        1,
        "A stopped being committed the moment it started playing"
    );
}
