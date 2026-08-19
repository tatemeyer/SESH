//! Closing launches the log never saw end.
//!
//! `Launcher` pairs `app.launched` with `app.exited` in memory. Restart the
//! daemon while an app is running and that pairing is gone: the reaper never
//! observes the exit, so it never records one, and the append-only log keeps a
//! launch that nothing will ever close. Rebuild "what is running" from such a
//! log and it reports an app that stopped hours ago.
//!
//! This is routine rather than exceptional — a config reload, a crash under
//! `Restart=always`, or someone pulling the plug on an appliance that lives in
//! a living room all produce one.
//!
//! At startup, `seshd` closes those launches. It cannot know *when* the app
//! went, so it does not pretend to: the closing event carries
//! `exit_observed: false` and both bounds the log can actually prove.

use anyhow::Result;
use serde_json::json;

use crate::event::{kind, Event, NewEvent};
use crate::room::Room;

/// Recorded on a closing event so consumers can tell an observed exit from an
/// inferred one. A projection computing session lengths must not treat an
/// inferred exit as a measurement.
pub const EXIT_OBSERVED: &str = "exit_observed";

/// Why the exit was never seen. Free text for humans reading the log.
const REASON: &str = "seshd restarted while this app was running";

/// The closing events a log needs to become self-consistent.
///
/// Walks `events` in order and returns one `app.exited` for every
/// `app.launched` still open at the end. Pure: no clock, no I/O. The store
/// assigns each returned event its `ts_ms`, which is the upper bound on when
/// the app died — by the time this is recorded, it certainly has.
///
/// **That bound holds only when the row is unmarked.** This Pi has no RTC, so
/// at a cold boot the wall clock can still be showing the last shutdown when
/// this runs, which puts `ts_ms` *below* the `last_alive_ms` in its own
/// payload. `main` waits a bounded ten seconds for the clock rather than
/// racing it, and a row written anyway carries
/// [`CLOCK_SYNCED`](crate::clock::CLOCK_SYNCED) `= false`. Read that key before
/// trusting the bound.
///
/// Returns an empty vector for a log that is already consistent, which makes
/// running it twice a no-op.
pub fn unfinished_launches(events: &[Event]) -> Vec<NewEvent> {
    // Subject -> id of the launch that is currently open for it. Ordered by
    // that id at the end so the output is deterministic across runs.
    let mut open: Vec<(String, i64)> = Vec::new();

    for event in events {
        // A launch or exit with no subject names no app, so there is nothing
        // to open or close.
        let Some(subject) = event.subject.as_deref() else {
            continue;
        };

        match event.kind.as_str() {
            kind::APP_LAUNCHED => {
                // Two launches with no exit between them should leave one open
                // entry, not two. The later one wins: it is the launch whose
                // exit is genuinely missing.
                open.retain(|(s, _)| s != subject);
                open.push((subject.to_string(), event.id));
            }
            kind::APP_EXITED => open.retain(|(s, _)| s != subject),
            _ => {}
        }
    }

    open.sort_by_key(|&(_, id)| id);

    // The newest event is the latest instant SESH can prove it was watching
    // and had not recorded an exit. Everything before it is a weaker bound.
    let last_alive_ms = events.last().map(|e| e.ts_ms);

    open.into_iter()
        .map(|(subject, _)| {
            let mut payload = json!({ EXIT_OBSERVED: false, "reason": REASON });
            if let Some(ms) = last_alive_ms {
                payload["last_alive_ms"] = json!(ms);
            }
            NewEvent::new(kind::APP_EXITED)
                .subject(subject)
                .payload(payload)
        })
        .collect()
}

/// Close any launch left open by a previous run, through the normal write
/// path. Returns the app ids closed, so the caller can say so out loud.
pub fn close_unfinished_launches(room: &Room) -> Result<Vec<String>> {
    let history = room.events_since(0, -1)?;
    let mut closed = Vec::new();

    for event in unfinished_launches(&history) {
        let subject = event.subject.clone();
        room.record(event)?;
        if let Some(subject) = subject {
            closed.push(subject);
        }
    }

    Ok(closed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::clock::{TestClock, CLOCK_SYNCED};
    use crate::store::Store;

    fn ev(id: i64, kind: &str, subject: Option<&str>) -> Event {
        Event {
            id,
            ts_ms: 1_700_000_000_000 + id,
            kind: kind.into(),
            actors: Vec::new(),
            subject: subject.map(Into::into),
            payload: json!({}),
        }
    }

    fn launched(id: i64, subject: &str) -> Event {
        ev(id, kind::APP_LAUNCHED, Some(subject))
    }

    fn exited(id: i64, subject: &str) -> Event {
        ev(id, kind::APP_EXITED, Some(subject))
    }

    fn subjects(events: &[NewEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| e.subject.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn an_empty_log_needs_nothing() {
        assert!(unfinished_launches(&[]).is_empty());
    }

    #[test]
    fn a_closed_launch_needs_nothing() {
        let log = [launched(1, "kodi"), exited(2, "kodi")];
        assert!(unfinished_launches(&log).is_empty());
    }

    #[test]
    fn an_open_launch_is_closed() {
        let log = [launched(1, "retroarch")];
        let out = unfinished_launches(&log);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, kind::APP_EXITED);
        assert_eq!(out[0].subject.as_deref(), Some("retroarch"));
    }

    #[test]
    fn a_closing_event_admits_it_did_not_see_the_exit() {
        let log = [launched(1, "retroarch")];
        let out = unfinished_launches(&log);

        // The whole point of the design: an inferred exit must be
        // distinguishable from an observed one, or duration statistics
        // silently absorb a number nobody measured.
        assert_eq!(
            out[0].payload[EXIT_OBSERVED],
            serde_json::Value::Bool(false)
        );
        assert_eq!(out[0].payload["last_alive_ms"], json!(1_700_000_000_001i64));
        assert!(out[0].payload["reason"].is_string());
    }

    #[test]
    fn last_alive_is_the_newest_event_not_the_launch() {
        // The launch is old; SESH was demonstrably still watching later.
        let log = [
            launched(1, "kodi"),
            ev(2, kind::PRESENCE_ARRIVED, None),
            ev(3, "match.result", Some("smash")),
        ];
        let out = unfinished_launches(&log);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload["last_alive_ms"], json!(1_700_000_000_003i64));
    }

    #[test]
    fn only_the_still_open_launch_is_closed() {
        let log = [
            launched(1, "kodi"),
            exited(2, "kodi"),
            launched(3, "retroarch"),
        ];
        assert_eq!(subjects(&unfinished_launches(&log)), vec!["retroarch"]);
    }

    #[test]
    fn relaunching_the_same_app_leaves_one_open_launch() {
        // Two launches, no exit between them — a restart during the second.
        // Closing it twice would invent an exit that never happened.
        let log = [launched(1, "kodi"), launched(2, "kodi")];
        assert_eq!(subjects(&unfinished_launches(&log)), vec!["kodi"]);
    }

    #[test]
    fn several_open_launches_are_closed_in_log_order() {
        let log = [
            launched(1, "kodi"),
            launched(2, "retroarch"),
            launched(3, "moonlight"),
            exited(4, "retroarch"),
        ];
        assert_eq!(
            subjects(&unfinished_launches(&log)),
            vec!["kodi", "moonlight"],
            "output must be deterministic, ordered by when each launch opened"
        );
    }

    #[test]
    fn unrelated_kinds_are_ignored() {
        let log = [
            ev(1, kind::PRESENCE_ARRIVED, Some("tate")),
            ev(2, "match.result", Some("smash")),
            ev(3, "music.queued", Some("track-1")),
        ];
        assert!(unfinished_launches(&log).is_empty());
    }

    #[test]
    fn launches_without_a_subject_name_no_app_to_close() {
        let log = [ev(1, kind::APP_LAUNCHED, None)];
        assert!(unfinished_launches(&log).is_empty());
    }

    fn room() -> std::sync::Arc<Room> {
        Room::new(Store::open_in_memory().unwrap()).unwrap()
    }

    /// A room whose clock the test drives, so a reboot into the pre-NTP window
    /// can be reproduced without rebooting.
    fn room_with_clock(wall_ms: i64) -> (std::sync::Arc<Room>, Arc<TestClock>) {
        let clock = Arc::new(TestClock::new(wall_ms));
        clock.set_synced(true);
        let store = Store::open_in_memory().unwrap().with_clock(clock.clone());
        (Room::new(store).unwrap(), clock)
    }

    // The measured failure, reproduced. An evening ends with Kodi running; the
    // Pi reboots; `seshd` comes up before NTP has answered, with the wall clock
    // back where it was at the last shutdown. The closing row's `ts_ms` is then
    // *below* the `last_alive_ms` in its own payload — the upper bound under
    // the lower one.
    //
    // Nothing can make that row's timestamp right, because the box genuinely
    // did not know the time. What it can do is not claim otherwise.
    #[test]
    fn a_row_written_before_the_clock_is_set_says_so() {
        let (room, clock) = room_with_clock(1_787_161_900_000);
        room.record(NewEvent::new(kind::APP_LAUNCHED).subject("kodi"))
            .unwrap();

        // Reboot: the clock falls back to the last shutdown and is not yet
        // trusted.
        clock.set_wall_ms(1_787_161_092_000);
        clock.set_synced(false);

        close_unfinished_launches(&room).unwrap();

        let log = room.events_since(0, -1).unwrap();
        let closing = log.last().unwrap();
        assert_eq!(closing.kind, kind::APP_EXITED);

        let last_alive = closing.payload["last_alive_ms"].as_i64().unwrap();
        assert!(
            closing.ts_ms < last_alive,
            "the contradiction is real and cannot be timestamped away"
        );
        assert_eq!(
            closing.payload[CLOCK_SYNCED], false,
            "so the row must admit the clock was not set when it was written"
        );
    }

    // And the ordinary case stays clean: a box that knows the time writes rows
    // that carry no apology.
    #[test]
    fn a_row_written_with_a_set_clock_carries_no_mark() {
        let (room, _clock) = room_with_clock(1_787_161_900_000);
        room.record(NewEvent::new(kind::APP_LAUNCHED).subject("kodi"))
            .unwrap();

        close_unfinished_launches(&room).unwrap();

        let log = room.events_since(0, -1).unwrap();
        let closing = log.last().unwrap();
        let last_alive = closing.payload["last_alive_ms"].as_i64().unwrap();

        assert!(
            closing.ts_ms >= last_alive,
            "the documented bound holds here"
        );
        assert!(closing.payload.get(CLOCK_SYNCED).is_none());
    }

    #[test]
    fn closing_goes_through_the_write_path_and_is_visible() {
        let room = room();
        room.record(NewEvent::new(kind::APP_LAUNCHED).subject("kodi"))
            .unwrap();

        let closed = close_unfinished_launches(&room).unwrap();
        assert_eq!(closed, vec!["kodi".to_string()]);

        let log = room.events_since(0, -1).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].kind, kind::APP_EXITED);
        assert_eq!(log[1].subject.as_deref(), Some("kodi"));
        assert_eq!(
            log[1].payload[EXIT_OBSERVED],
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn closing_twice_appends_nothing_the_second_time() {
        let room = room();
        room.record(NewEvent::new(kind::APP_LAUNCHED).subject("kodi"))
            .unwrap();

        close_unfinished_launches(&room).unwrap();
        let after_first = room.events_since(0, -1).unwrap().len();

        let closed = close_unfinished_launches(&room).unwrap();
        assert!(closed.is_empty());
        assert_eq!(
            room.events_since(0, -1).unwrap().len(),
            after_first,
            "reconciliation must be idempotent — a restart loop must not \
             append a closing event every time"
        );
    }

    #[test]
    fn a_consistent_log_is_left_alone() {
        let room = room();
        room.record(NewEvent::new(kind::APP_LAUNCHED).subject("kodi"))
            .unwrap();
        room.record(NewEvent::new(kind::APP_EXITED).subject("kodi"))
            .unwrap();

        assert!(close_unfinished_launches(&room).unwrap().is_empty());
        assert_eq!(room.events_since(0, -1).unwrap().len(), 2);
    }
}
