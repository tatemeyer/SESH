//! The event type. Everything in SESH is an event or a view of events.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A recorded fact about the room. Immutable once appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic, never reused. Assigned by the store.
    pub id: i64,
    /// Unix milliseconds. Assigned by the store.
    pub ts_ms: i64,
    /// Dotted kind, e.g. `presence.arrived`. Free-form on purpose: future
    /// producers add kinds without a schema migration.
    pub kind: String,
    /// Person ids this event is about.
    #[serde(default)]
    pub actors: Vec<String>,
    /// What the event acted on — an app id, a game, a track.
    #[serde(default)]
    pub subject: Option<String>,
    /// Kind-specific detail.
    #[serde(default)]
    pub payload: Value,
}

/// An event that has not been appended yet — no id, no timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    /// Dotted kind, e.g. `app.launched`.
    pub kind: String,
    /// Person ids this event is about.
    #[serde(default)]
    pub actors: Vec<String>,
    /// What the event acted on.
    #[serde(default)]
    pub subject: Option<String>,
    /// Kind-specific detail. Defaults to an empty object.
    #[serde(default = "empty_payload")]
    pub payload: Value,
}

fn empty_payload() -> Value {
    Value::Object(serde_json::Map::new())
}

impl NewEvent {
    /// Start building an event of the given kind.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            actors: Vec::new(),
            subject: None,
            payload: empty_payload(),
        }
    }

    /// Add a person this event is about.
    pub fn actor(mut self, id: impl Into<String>) -> Self {
        self.actors.push(id.into());
        self
    }

    /// Set what the event acted on.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set kind-specific detail.
    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
}

/// Event kinds SESH itself emits. `Event::kind` is a free string by design;
/// these are the ones this codebase produces.
pub mod kind {
    /// Someone joined the room.
    pub const PRESENCE_ARRIVED: &str = "presence.arrived";
    /// Someone left the room.
    pub const PRESENCE_LEFT: &str = "presence.left";
    /// An app was started by the launcher.
    pub const APP_LAUNCHED: &str = "app.launched";
    /// An app stopped — quit by SESH or exited on its own.
    pub const APP_EXITED: &str = "app.exited";
    /// Someone scanned the QR and became a person the house knows.
    pub const PERSON_JOINED: &str = "person.joined";

    // Music. `subject` is always the track URI, so "what did we play the night
    // Marcus came over" reads naturally off the log. *Identity* of a queue
    // entry is `payload.entry` — the id of the `music.queued` event — because
    // two people queueing the same song must get two independently vetoable
    // entries, and on a shared-queue night that is a running joke, not an
    // edge case.

    /// Someone added a track. The actor is who added it; this event's own id
    /// becomes the queue entry id every later music event refers to.
    pub const MUSIC_QUEUED: &str = "music.queued";
    /// Someone voted to skip a queued or playing track.
    pub const MUSIC_VETOED: &str = "music.vetoed";
    /// A track began playing.
    pub const MUSIC_STARTED: &str = "music.started";
    /// A track ended or was dropped. `payload.why` says which.
    pub const MUSIC_SKIPPED: &str = "music.skipped";

    /// The room's Bluetooth speaker appeared. `subject` is the sink name.
    pub const AUDIO_SINK_FOUND: &str = "audio.sink_found";
    /// The room's Bluetooth speaker went away. `subject` is the sink name.
    ///
    /// On a Victrola this is also the vinyl handoff signal: switching the unit
    /// to phono drops the A2DP link, so "the speaker left" and "someone put a
    /// record on" are the same event.
    pub const AUDIO_SINK_LOST: &str = "audio.sink_lost";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_event_builder_sets_fields() {
        let e = NewEvent::new(kind::APP_LAUNCHED)
            .actor("tate")
            .subject("kodi")
            .payload(serde_json::json!({ "via": "controller" }));

        assert_eq!(e.kind, "app.launched");
        assert_eq!(e.actors, vec!["tate".to_string()]);
        assert_eq!(e.subject.as_deref(), Some("kodi"));
        assert_eq!(e.payload["via"], "controller");
    }

    #[test]
    fn event_round_trips_through_json() {
        let e = Event {
            id: 7,
            ts_ms: 1_700_000_000_000,
            kind: "presence.arrived".into(),
            actors: vec!["sam".into()],
            subject: None,
            payload: serde_json::json!({}),
        };
        let back: Event = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn new_event_deserializes_with_only_a_kind() {
        let e: NewEvent = serde_json::from_str(r#"{"kind":"match.result"}"#).unwrap();
        assert_eq!(e.kind, "match.result");
        assert!(e.actors.is_empty());
        assert_eq!(e.subject, None);
        assert_eq!(e.payload, serde_json::json!({}));
    }
}
