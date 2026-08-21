//! Presence from a bonded phone or tag.
//!
//! The strongest `via` in the fusion, and the only one that distinguishes *this
//! room* from *this apartment*. Everything else here follows from one
//! measurement: **a passive scan in this flat sees ~340 addresses in nineteen
//! minutes, 56% of them rotating, and only 18 of 340 ever give a name.**
//!
//! ## Why a bond
//!
//! iOS and Android rotate their BLE advertising address to defeat exactly the
//! thing a room like this would otherwise do. A Resolvable Private Address can
//! only be tied back to a device by something holding its Identity Resolving
//! Key, and an IRK is exchanged during a real bond. So a bonded phone is
//! recognisable and an unbonded one is, correctly, not.
//!
//! Everybody already walks in holding a phone, so this costs no purchase and no
//! carried object — one pairing per person, once, not per visit. A tag is the
//! same thing in a different shape and is equally supported: the fusion cannot
//! tell them apart and does not need to. Both are [`Via::Ble`].
//!
//! ## Match, never enumerate
//!
//! This module compares what it sees against the people who chose to enrol, and
//! ignores everything else absolutely. It never builds a list of what is in
//! range. That is a surveillance device, and at 340 addresses per nineteen
//! minutes it is also useless.
//!
//! The one list of addresses SESH holds is `people.bt_identity`, and everybody
//! in it put themselves there.

use std::collections::BTreeMap;

use anyhow::Result;

use super::fusion::Fusion;
use super::via::Via;

/// One observation of one device, as a scanner reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// The device's identity address, upper case.
    ///
    /// For a bonded device this is the *identity* address rather than whatever
    /// rotating address was on the air, because resolving the one to the other
    /// is what the bond is for.
    pub address: String,
    /// Signal strength, when the scanner reported one.
    ///
    /// `None` means the device is known but was not heard from — a cached entry
    /// rather than a sighting. Treating those as presence would put every phone
    /// ever bonded in the room forever.
    pub rssi: Option<i64>,
}

/// Where sightings come from.
///
/// A trait so the logic above it runs with no radio, and so the double can fail
/// the way the real thing does — a scan can refuse, and BlueZ in particular can
/// wedge such that discovery silently reports nothing at all.
pub trait BondedScan: Send + Sync {
    /// Devices heard from since the last look.
    ///
    /// An `Err` means *the scan failed*, which is not the same as *nobody is
    /// here* and must never be recorded as such.
    fn sightings(&self) -> Result<Vec<Sighting>>;
}

/// Turns sightings of enrolled devices into presence.
///
/// Holds the enrolment map — address to person — and nothing else. Rebuilt from
/// `people` rather than cached indefinitely, so enrolling a phone takes effect
/// without a restart.
#[derive(Debug, Default)]
pub struct BleWatch {
    enrolled: BTreeMap<String, String>,
}

impl BleWatch {
    /// A watch over these enrolments, as `(address, person id)`.
    pub fn new(enrolled: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            enrolled: enrolled
                .into_iter()
                .map(|(address, person)| (address.to_uppercase(), person))
                .collect(),
        }
    }

    /// Whether anybody is enrolled at all.
    ///
    /// Nobody being enrolled is the ordinary state on the day this ships, and
    /// it is not an error: everyone stays on `heartbeat` until they bond. The
    /// arc lands one phone at a time and there is no flag day.
    pub fn is_empty(&self) -> bool {
        self.enrolled.is_empty()
    }
}

/// Fold one round of sightings into the fusion.
///
/// Only enrolled addresses that were actually heard from become presence.
/// Everything else is dropped without being counted, named, or logged.
///
/// Returns the people seen, for the caller to log or ignore.
pub fn observe(
    watch: &BleWatch,
    fusion: &mut Fusion,
    sightings: &[Sighting],
    now_ms: i64,
) -> Vec<String> {
    let mut seen = Vec::new();
    for sighting in sightings {
        // No RSSI means BlueZ knows the device, not that it is here.
        if sighting.rssi.is_none() {
            continue;
        }
        let Some(person) = watch.enrolled.get(&sighting.address.to_uppercase()) else {
            // Not enrolled. Not a stranger, not a guest, not a row in anything:
            // nothing at all. This `continue` is the privacy property.
            continue;
        };
        fusion.seen(person, &Via::Ble, now_ms);
        if !seen.iter().any(|already| already == person) {
            seen.push(person.clone());
        }
    }
    seen
}

/// The people an enrolled-but-unseen device belongs to.
///
/// Called only when a scan actually completed. A scan that *failed* must not
/// reach this: "we looked and you were not there" is evidence, "we could not
/// look" is not, and the fusion treats the two completely differently.
pub fn missing(watch: &BleWatch, fusion: &mut Fusion, sightings: &[Sighting], now_ms: i64) {
    let heard: Vec<String> = sightings
        .iter()
        .filter(|s| s.rssi.is_some())
        .map(|s| s.address.to_uppercase())
        .collect();

    for (address, person) in &watch.enrolled {
        if !heard.contains(address) {
            fusion.missing(person, &Via::Ble, now_ms);
        }
    }
}

#[cfg(test)]
#[path = "ble_tests.rs"]
mod tests;
