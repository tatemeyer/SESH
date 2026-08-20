//! Synthetic evenings.
//!
//! The plan asks for the fusion rules to be proven over seeded timelines rather
//! than argued about, because BLE is the one part of this arc a suite cannot
//! see. Everything here runs in microseconds with no radio.

use super::fusion::*;
use super::via::Via;

const T0: i64 = 1_786_937_604_000;
const MIN: i64 = 60 * 1000;

fn at(minutes: i64) -> i64 {
    T0 + minutes * MIN
}

#[test]
fn nobody_is_present_in_an_empty_room() {
    let fusion = Fusion::new();
    assert!(fusion.present(T0).is_empty());
    assert_eq!(fusion.via_for("sam", T0), None);
}

#[test]
fn one_signal_is_enough_to_be_here() {
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Heartbeat, T0);

    assert_eq!(fusion.present(T0), vec!["sam".to_string()]);
    assert_eq!(fusion.via_for("sam", T0), Some(Via::Heartbeat));
}

#[test]
fn a_signal_stops_counting_once_its_own_window_passes() {
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Ble, T0);

    // BLE's window is short: a tag not heard from in minutes has gone.
    let window = window_ms(&Via::Ble);
    assert!(fusion.is_present("sam", T0 + window - 1));
    assert!(!fusion.is_present("sam", T0 + window));
}

#[test]
fn the_windows_actually_differ_per_signal() {
    // If these ever collapse to one number, the whole reason for keeping `via`
    // on the row has quietly evaporated.
    let ble = window_ms(&Via::Ble);
    let heartbeat = window_ms(&Via::Heartbeat);
    let wifi = window_ms(&Via::Wifi);
    let asserted = window_ms(&Via::Asserted);

    assert!(ble < heartbeat, "a BLE gap means more than a heartbeat gap");
    assert!(heartbeat < wifi, "a lease outlives a dozing phone");
    assert!(wifi < asserted, "a human's word outlasts a lease");
}

#[test]
fn presence_is_the_union_of_live_signals() {
    // Sam's phone locked twenty minutes ago; his tag is still in the room.
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Heartbeat, T0);
    fusion.seen("sam", &Via::Ble, at(20));

    assert!(
        fusion.is_present("sam", at(20)),
        "the heartbeat expiring does not remove someone a radio can see"
    );
    assert_eq!(fusion.via_for("sam", at(20)), Some(Via::Ble));
}

#[test]
fn a_stale_strong_signal_does_not_evict_a_live_weak_one() {
    // The failure this rule exists to prevent: the BLE scanner dies. Silence
    // from a crashed radio looks exactly like silence from an empty room, so
    // it must not be read as evidence of either.
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Ble, T0);
    fusion.seen("sam", &Via::Heartbeat, at(20));

    assert!(
        fusion.is_present("sam", at(20)),
        "a scanner going quiet must not empty a room full of people"
    );
    assert_eq!(fusion.via_for("sam", at(20)), Some(Via::Heartbeat));
}

#[test]
fn a_strong_signal_that_looked_and_found_nothing_does_evict() {
    // The other half: the scan ran, and Sam's tag was not in it. That is
    // evidence, and it outranks a phone with a tab left open.
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Heartbeat, T0);
    fusion.missing("sam", &Via::Ble, T0);

    assert!(
        !fusion.is_present("sam", T0),
        "a tab left open is not a person"
    );
}

#[test]
fn a_missing_verdict_stops_counting_once_it_ages_out() {
    // The sharpest edge of "positively absent, never merely stale". A sweep
    // said "not here", which was evidence at the time. Once that verdict is
    // older than BLE's own window it is not evidence any more, and it must not
    // hold someone out of the room forever on the strength of one old look.
    let mut fusion = Fusion::new();
    fusion.missing("sam", &Via::Ble, T0);
    fusion.seen("sam", &Via::Heartbeat, T0);
    assert!(!fusion.is_present("sam", T0), "the fresh verdict evicts");

    // Five minutes on, the BLE verdict has outlived its window and Sam's phone
    // is still beating. Nothing currently says he is absent.
    fusion.seen("sam", &Via::Heartbeat, at(5));
    assert!(
        fusion.is_present("sam", at(5)),
        "one stale look must not exile someone for the rest of the evening"
    );
    assert_eq!(fusion.via_for("sam", at(5)), Some(Via::Heartbeat));
}

#[test]
fn a_weak_signal_looking_and_failing_does_not_evict_a_strong_one() {
    // Wifi cannot overrule the radio that can actually see the room.
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Ble, T0);
    fusion.missing("sam", &Via::Wifi, T0);

    assert!(fusion.is_present("sam", T0));
    assert_eq!(fusion.via_for("sam", T0), Some(Via::Ble));
}

#[test]
fn a_human_outranks_every_radio_in_the_room() {
    let mut fusion = Fusion::new();
    fusion.missing("marcus", &Via::Ble, T0);
    fusion.missing("marcus", &Via::Wifi, T0);
    fusion.seen("marcus", &Via::Asserted, T0);

    assert!(
        fusion.is_present("marcus", T0),
        "a room that cannot be told it is wrong becomes infuriating"
    );
    assert_eq!(fusion.via_for("marcus", T0), Some(Via::Asserted));
}

#[test]
fn an_assertion_expires_rather_than_standing_forever() {
    let mut fusion = Fusion::new();
    fusion.seen("marcus", &Via::Asserted, T0);

    let window = window_ms(&Via::Asserted);
    assert!(fusion.is_present("marcus", T0 + window - 1));
    assert!(
        !fusion.is_present("marcus", T0 + window),
        "an assertion that never expires is a lie with a long half-life"
    );
}

#[test]
fn an_unknown_signal_is_believed_but_never_wins_an_argument() {
    // An open ingest port means producers this build has never heard of. They
    // count, at the weakest rank, and cannot overrule anything we do know.
    let doorbell = Via::Other("doorbell".to_string());
    let mut fusion = Fusion::new();
    fusion.seen("sam", &doorbell, T0);
    assert!(fusion.is_present("sam", T0));

    fusion.missing("sam", &Via::Ble, T0);
    assert!(
        !fusion.is_present("sam", T0),
        "a signal we do not understand cannot outrank one we do"
    );
}

#[test]
fn people_are_decided_independently() {
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Ble, T0);
    fusion.missing("jess", &Via::Ble, T0);
    fusion.seen("jess", &Via::Heartbeat, T0);

    assert_eq!(fusion.present(T0), vec!["sam".to_string()]);
}

/// The shape the plan actually asks for: an evening, replayed.
///
/// Walked strictly forwards. Signals are seeded only up to the moment being
/// asserted, never past it — an earlier draft seeded the whole timeline first
/// and then asked about 20:40, at which point the stored tag sighting was
/// timestamped 21:00 and the test was quietly reading the future. A test that
/// can see ahead proves nothing about a room that cannot.
#[test]
fn a_synthetic_evening() {
    let mut fusion = Fusion::new();

    // A tag in the room is seen on every sweep and a phone in a hand beats on
    // every tick, so seed once a minute over the interval that has elapsed.
    fn sweep(fusion: &mut Fusion, person: &str, via: &Via, from: i64, to: i64) {
        for m in from..=to {
            fusion.seen(person, via, at(m));
        }
    }

    // 20:00 — Tate is home. Tag in his pocket, phone in his hand.
    sweep(&mut fusion, "tate", &Via::Ble, 0, 0);
    sweep(&mut fusion, "tate", &Via::Heartbeat, 0, 0);
    assert_eq!(fusion.present(at(0)), vec!["tate".to_string()]);

    // 20:10 — Sam arrives with no tag. His phone is all the room has of him.
    sweep(&mut fusion, "tate", &Via::Ble, 1, 10);
    sweep(&mut fusion, "tate", &Via::Heartbeat, 1, 10);
    sweep(&mut fusion, "sam", &Via::Heartbeat, 10, 10);
    assert_eq!(
        fusion.present(at(10)),
        vec!["sam".to_string(), "tate".to_string()]
    );

    // 20:19 — Sam pocketed his phone nine minutes ago. Stale is not gone, and
    // nothing has looked for him and failed, so he is still in the room.
    sweep(&mut fusion, "tate", &Via::Ble, 11, 19);
    sweep(&mut fusion, "tate", &Via::Heartbeat, 11, 19);
    assert!(fusion.is_present("sam", at(19)));

    // 20:25 — Tate's phone goes down. Nothing beats for it after this.
    sweep(&mut fusion, "tate", &Via::Ble, 20, 25);
    sweep(&mut fusion, "tate", &Via::Heartbeat, 20, 25);

    // 20:40 — Sam's heartbeat has outlived its window and nothing else ever
    // saw him, so the room stops believing in him. That is the degraded mode
    // working as described, and it is the argument for carrying a tag.
    sweep(&mut fusion, "tate", &Via::Ble, 26, 40);
    assert!(!fusion.is_present("sam", at(40)));

    // Tate's dead phone changes nothing: the tag is still being seen, and it
    // is the stronger signal anyway.
    assert!(fusion.is_present("tate", at(40)));
    assert_eq!(fusion.via_for("tate", at(40)), Some(Via::Ble));

    // 21:00 — the sweep runs and Tate's tag is not in it. He has gone to bed.
    sweep(&mut fusion, "tate", &Via::Ble, 41, 59);
    fusion.missing("tate", &Via::Ble, at(60));
    assert!(!fusion.is_present("tate", at(60)));
    assert!(fusion.present(at(60)).is_empty());
}

#[test]
fn forgetting_old_signals_does_not_change_who_is_here() {
    let mut fusion = Fusion::new();
    fusion.seen("sam", &Via::Heartbeat, T0);
    fusion.seen("tate", &Via::Ble, at(120));

    let before = fusion.present(at(120));
    fusion.forget_before(at(60));

    assert_eq!(fusion.present(at(120)), before);
    assert_eq!(fusion.present(at(120)), vec!["tate".to_string()]);
}
