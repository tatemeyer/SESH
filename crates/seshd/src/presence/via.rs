//! How the room knows someone is here.
//!
//! Presence used to mean exactly one thing — a phone with the SESH tab open and
//! foregrounded — so a `presence.arrived` row needed no explanation. Once BLE,
//! wifi and a human's own say-so can all produce the same row, the row has to
//! say which one it was, or a reader cannot tell a person in the room from a
//! browser tab left open in a coat pocket two streets away.
//!
//! Fourth instance of the house idiom, after `exit_observed` in
//! [`reconcile`](crate::reconcile), `clock_synced` in [`clock`](crate::clock),
//! and `entry` on the music rows: **record what you know about how you know it,
//! rather than flattening it into the thing you wish you knew.**
//!
//! Unlike `exit_observed` and `clock_synced`, which are written only when the
//! news is bad, `via` is written on every row this build emits. Absent means
//! unknown, and unknown is exactly what every row written before this existed
//! is — there is no backfill and there will not be one.

use serde::{Deserialize, Serialize};

use crate::event::Event;

/// Payload key naming the signal a presence row came from.
pub const VIA: &str = "via";

/// Payload key carrying signal strength, for the signals that have one.
///
/// Kept as the raw reading rather than a bucketed confidence: what counts as
/// "in this room" is a property of one flat's walls and is tuned against a real
/// room, so the log records the measurement and projections do the judging.
pub const RSSI: &str = "rssi";

/// The signal behind a `presence.arrived` or `presence.left`.
///
/// Deliberately a closed set plus an escape hatch. `POST /api/events` is an open
/// ingest port, so a producer this build has never heard of must be able to post
/// a presence row and have it survive intact — [`Via::Other`] carries that value
/// through rather than rejecting it or, worse, silently reading it as something
/// it is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Via {
    /// A proximity radio saw something this person carries.
    ///
    /// The only signal that distinguishes *this room* from *this apartment*,
    /// which is why the arc bothers with it at all.
    Ble,
    /// This person's device is on the house network. Cheap and passive, and it
    /// knows the flat rather than the room.
    Wifi,
    /// This person's phone has SESH open and is beating. The floor, and the
    /// degraded mode the vision always described.
    Heartbeat,
    /// A human told the room, and the room believes them over its own senses.
    ///
    /// The escape hatch, and not optional: a room that can be told it is wrong
    /// never becomes infuriating.
    Asserted,
    /// A signal this build does not know about, preserved verbatim.
    Other(String),
}

impl Via {
    /// The wire form, as it appears in a payload.
    pub fn as_str(&self) -> &str {
        match self {
            Via::Ble => "ble",
            Via::Wifi => "wifi",
            Via::Heartbeat => "heartbeat",
            Via::Asserted => "asserted",
            Via::Other(raw) => raw,
        }
    }

    /// Read the `via` off an event's payload.
    ///
    /// `None` means the row does not say — either it predates this field, or it
    /// came from a producer that knows nothing about it. Both are *unknown*, and
    /// a reader must not promote unknown to any particular signal.
    pub fn read(event: &Event) -> Option<Self> {
        event.payload.get(VIA)?.as_str().map(Self::from)
    }

    /// Read the `rssi` off an event's payload, when it has one.
    pub fn rssi(event: &Event) -> Option<i64> {
        event.payload.get(RSSI)?.as_i64()
    }
}

impl From<&str> for Via {
    fn from(raw: &str) -> Self {
        match raw {
            "ble" => Via::Ble,
            "wifi" => Via::Wifi,
            "heartbeat" => Via::Heartbeat,
            "asserted" => Via::Asserted,
            other => Via::Other(other.to_string()),
        }
    }
}

impl From<String> for Via {
    fn from(raw: String) -> Self {
        Via::from(raw.as_str())
    }
}

impl From<Via> for String {
    fn from(via: Via) -> Self {
        via.as_str().to_string()
    }
}

impl std::fmt::Display for Via {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::kind;

    fn row(payload: serde_json::Value) -> Event {
        Event {
            id: 1,
            ts_ms: 0,
            kind: kind::PRESENCE_ARRIVED.into(),
            actors: vec!["tate".into()],
            subject: None,
            payload,
        }
    }

    #[test]
    fn the_four_known_signals_round_trip() {
        for via in [Via::Ble, Via::Wifi, Via::Heartbeat, Via::Asserted] {
            let back = Via::from(via.as_str());
            assert_eq!(via, back, "{via} did not survive a round trip");
        }
    }

    #[test]
    fn a_row_with_no_via_reads_as_unknown() {
        // Every one of the rows written before this field existed. They must
        // read as "does not say", never as heartbeat, even though heartbeat is
        // what every one of them in fact was.
        assert_eq!(Via::read(&row(serde_json::json!({}))), None);
    }

    #[test]
    fn an_unrecognised_via_is_preserved_rather_than_dropped() {
        // POST /api/events is an open ingest port. A producer this build has
        // never heard of must not have its provenance quietly erased.
        let event = row(serde_json::json!({ VIA: "seismograph" }));
        assert_eq!(
            Via::read(&event),
            Some(Via::Other("seismograph".to_string()))
        );
        assert_eq!(Via::read(&event).unwrap().as_str(), "seismograph");
    }

    #[test]
    fn a_via_that_is_not_a_string_reads_as_unknown() {
        // Rather than panicking on a payload some other producer shaped
        // differently.
        assert_eq!(Via::read(&row(serde_json::json!({ VIA: 7 }))), None);
    }

    #[test]
    fn rssi_is_read_when_present_and_absent_otherwise() {
        let with = row(serde_json::json!({ VIA: "ble", RSSI: -58 }));
        assert_eq!(Via::rssi(&with), Some(-58));
        assert_eq!(Via::rssi(&row(serde_json::json!({ VIA: "ble" }))), None);
    }

    #[test]
    fn via_serialises_as_a_bare_string() {
        // It rides inside a payload object, so it must not become {"Ble": null}
        // or any other adjacently-tagged shape.
        assert_eq!(serde_json::to_string(&Via::Ble).unwrap(), r#""ble""#);
        assert_eq!(
            serde_json::from_str::<Via>(r#""asserted""#).unwrap(),
            Via::Asserted
        );
    }
}
