//! Tests for the presence tracker.
//!
//! Split out of `mod.rs` to keep that file under the 300-line ceiling once
//! attention and presence became two questions instead of one.

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

// --- attention is not presence -------------------------------------------
//
// Arc 3 Phase 2. These two questions were one number for as long as the
// heartbeat was the only signal, and the whole arc turns on separating them.

#[test]
fn a_locked_screen_ends_attention_and_says_nothing_about_presence() {
    let presence = Presence::new();
    presence.beat("sam", T0);

    // Still beating: both questions say yes.
    assert_eq!(presence.attentive(T0), vec!["sam".to_string()]);
    assert_eq!(presence.present(T0), vec!["sam".to_string()]);

    // Phone face-down on the couch, four beers into a Smash set. Attention is
    // gone within the minute. Sam has not moved.
    let later = T0 + ATTENTION_MS;
    assert!(
        presence.attentive(later).is_empty(),
        "a quiet phone is not paying attention"
    );
    assert_eq!(
        presence.present(later),
        vec!["sam".to_string()],
        "a quiet phone is not evidence its owner left the room"
    );
}

#[test]
fn presence_still_ends_at_the_window() {
    // Attention being the shorter question must not make presence unbounded:
    // the tab left open by someone who went home an hour ago still expires.
    let presence = Presence::new();
    presence.beat("sam", T0);

    assert!(presence.present(T0 + WINDOW_MS - 1).len() == 1);
    assert!(presence.present(T0 + WINDOW_MS).is_empty());
}

#[test]
fn the_veto_denominator_follows_presence_not_attention() {
    // The bug this arc is named after: a majority of the people currently
    // staring at their phones is not a majority of the room.
    //
    // Five people in the room, two still holding their phones. A veto needs
    // three. Read off attention it would need two, and two people could skip a
    // track for five — which is not a majority of anything.
    //
    // The counts must differ for this test to be worth anything: with a small
    // room MIN_VOTES floors both answers to 2 and the assertion proves nothing.
    let presence = Presence::new();
    for who in ["sam", "jess", "marcus", "ali", "tate"] {
        presence.beat(who, T0);
    }
    let later = T0 + ATTENTION_MS;
    presence.beat("sam", later);
    presence.beat("jess", later);

    let attentive = presence.attentive(later);
    let present = presence.present(later);

    assert_eq!(attentive, vec!["jess".to_string(), "sam".to_string()]);
    assert_eq!(present.len(), 5, "nobody has left the room");

    assert_eq!(
        crate::veto::needed(&present),
        3,
        "five in the room is a majority of three"
    );
    assert_eq!(
        crate::veto::needed(&attentive),
        2,
        "two on their phones would be a majority of two — the wrong number"
    );
    assert_ne!(
        crate::veto::needed(&present),
        crate::veto::needed(&attentive),
        "if these ever agree this test is vacuous"
    );
}

#[test]
fn someone_retired_is_neither_present_nor_attentive() {
    let presence = Presence::new();
    presence.beat("sam", T0);
    presence.sweep(T0 + WINDOW_MS);

    assert!(presence.present(T0 + WINDOW_MS).is_empty());
    assert!(presence.attentive(T0 + WINDOW_MS).is_empty());
}
