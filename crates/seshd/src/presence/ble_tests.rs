//! Tests for BLE presence.
//!
//! No radio. The scanner sits behind [`BondedScan`] precisely so the rules
//! above it are decided here, where an evening runs in microseconds.

use super::*;
use crate::presence::fusion::Fusion;

const T0: i64 = 1_786_937_604_000;

/// Placeholder addresses. Deliberately not anyone's real device — the repo is
/// public, and a Bluetooth identity address is a durable handle on a person.
const TATE_PHONE: &str = "AA:BB:CC:00:00:01";
const SAM_PHONE: &str = "AA:BB:CC:00:00:02";
const A_STRANGER: &str = "AA:BB:CC:FF:FF:FF";

fn watch() -> BleWatch {
    BleWatch::new([
        (TATE_PHONE.to_string(), "tate".to_string()),
        (SAM_PHONE.to_string(), "sam".to_string()),
    ])
}

fn seen_at(address: &str, rssi: i64) -> Sighting {
    Sighting {
        address: address.to_string(),
        rssi: Some(rssi),
    }
}

fn cached(address: &str) -> Sighting {
    Sighting {
        address: address.to_string(),
        rssi: None,
    }
}

#[test]
fn an_enrolled_phone_that_is_heard_is_present() {
    let mut fusion = Fusion::new();
    let seen = observe(&watch(), &mut fusion, &[seen_at(TATE_PHONE, -58)], T0);

    assert_eq!(seen, vec!["tate".to_string()]);
    assert!(fusion.is_present("tate", T0));
    assert_eq!(fusion.via_for("tate", T0), Some(Via::Ble));
}

/// The privacy property, and the reason this module is not a scanner.
#[test]
fn a_device_nobody_enrolled_is_nothing_at_all() {
    let mut fusion = Fusion::new();
    let seen = observe(
        &watch(),
        &mut fusion,
        &[seen_at(A_STRANGER, -40), seen_at(TATE_PHONE, -58)],
        T0,
    );

    assert_eq!(
        seen,
        vec!["tate".to_string()],
        "an unenrolled device must not appear anywhere, under any name"
    );
    assert_eq!(
        fusion.present(T0),
        vec!["tate".to_string()],
        "and must not become a person"
    );
}

/// At ~340 addresses per nineteen minutes in this flat, the cost of getting
/// this wrong is not theoretical.
#[test]
fn a_crowded_room_produces_only_the_people_in_it() {
    let mut fusion = Fusion::new();
    let mut crowd: Vec<Sighting> = (0..340)
        .map(|n| seen_at(&format!("DE:AD:00:00:{:02X}:{:02X}", n / 256, n % 256), -70))
        .collect();
    crowd.push(seen_at(SAM_PHONE, -55));

    let seen = observe(&watch(), &mut fusion, &crowd, T0);
    assert_eq!(seen, vec!["sam".to_string()]);
    assert_eq!(fusion.present(T0), vec!["sam".to_string()]);
}

#[test]
fn a_known_but_unheard_device_is_not_a_sighting() {
    // BlueZ keeps devices it has ever seen. Reading a cached entry as presence
    // would put every phone ever bonded in the room permanently.
    let mut fusion = Fusion::new();
    let seen = observe(&watch(), &mut fusion, &[cached(TATE_PHONE)], T0);

    assert!(seen.is_empty());
    assert!(!fusion.is_present("tate", T0));
}

#[test]
fn addresses_match_regardless_of_case() {
    let mut fusion = Fusion::new();
    let lower = TATE_PHONE.to_lowercase();
    let seen = observe(&watch(), &mut fusion, &[seen_at(&lower, -58)], T0);

    assert_eq!(seen, vec!["tate".to_string()]);
}

/// A completed scan that did not hear a phone is evidence. This is the half
/// that lets BLE outrank a heartbeat.
#[test]
fn a_completed_scan_that_missed_someone_says_so() {
    let mut fusion = Fusion::new();
    let watch = watch();

    observe(&watch, &mut fusion, &[seen_at(TATE_PHONE, -58)], T0);
    observe(&watch, &mut fusion, &[seen_at(SAM_PHONE, -60)], T0);
    assert_eq!(fusion.present(T0).len(), 2);

    // A later sweep hears only Tate. Sam has left.
    let later = T0 + 1000;
    observe(&watch, &mut fusion, &[seen_at(TATE_PHONE, -58)], later);
    missing(&watch, &mut fusion, &[seen_at(TATE_PHONE, -58)], later);

    assert!(fusion.is_present("tate", later));
    assert!(
        !fusion.is_present("sam", later),
        "a sweep that ran and did not find Sam is evidence Sam is gone"
    );
}

/// Nobody enrolled is the ordinary state on day one, not an error. The arc
/// lands one phone at a time.
#[test]
fn nobody_enrolled_is_a_quiet_no_op() {
    let empty = BleWatch::new([]);
    assert!(empty.is_empty());

    let mut fusion = Fusion::new();
    let seen = observe(&empty, &mut fusion, &[seen_at(A_STRANGER, -40)], T0);

    assert!(seen.is_empty());
    assert!(fusion.present(T0).is_empty());
}
