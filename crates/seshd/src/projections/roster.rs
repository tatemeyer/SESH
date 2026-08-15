//! Who is in the room right now. The first projection — the shape every
//! later one (trophy case, brackets, streaks) follows.

use std::collections::BTreeSet;

use crate::event::{kind, Event};
use crate::projection::Projection;

/// Who is in the room right now.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Roster {
    present: BTreeSet<String>,
}

impl Roster {
    /// Person ids currently in the room, in stable alphabetical order.
    pub fn present(&self) -> Vec<String> {
        self.present.iter().cloned().collect()
    }
}

impl Projection for Roster {
    fn apply(&mut self, event: &Event) {
        match event.kind.as_str() {
            kind::PRESENCE_ARRIVED => {
                for actor in &event.actors {
                    self.present.insert(actor.clone());
                }
            }
            kind::PRESENCE_LEFT => {
                for actor in &event.actors {
                    self.present.remove(actor);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

    fn ev(id: i64, kind: &str, actor: &str) -> Event {
        Event {
            id,
            ts_ms: 1_700_000_000_000 + id,
            kind: kind.into(),
            actors: vec![actor.into()],
            subject: None,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn empty_roster_has_nobody() {
        assert!(Roster::default().present().is_empty());
    }

    #[test]
    fn arriving_adds_a_person() {
        let r = Roster::rebuild(&[ev(1, kind::PRESENCE_ARRIVED, "tate")]);
        assert_eq!(r.present(), vec!["tate".to_string()]);
    }

    #[test]
    fn leaving_removes_a_person() {
        let r = Roster::rebuild(&[
            ev(1, kind::PRESENCE_ARRIVED, "tate"),
            ev(2, kind::PRESENCE_ARRIVED, "sam"),
            ev(3, kind::PRESENCE_LEFT, "tate"),
        ]);
        assert_eq!(r.present(), vec!["sam".to_string()]);
    }

    #[test]
    fn arriving_twice_does_not_duplicate() {
        let r = Roster::rebuild(&[
            ev(1, kind::PRESENCE_ARRIVED, "tate"),
            ev(2, kind::PRESENCE_ARRIVED, "tate"),
        ]);
        assert_eq!(r.present(), vec!["tate".to_string()]);
    }

    #[test]
    fn unrelated_events_are_ignored() {
        let r = Roster::rebuild(&[
            ev(1, kind::PRESENCE_ARRIVED, "tate"),
            ev(2, kind::APP_LAUNCHED, "tate"),
            ev(3, "match.result", "tate"),
        ]);
        assert_eq!(r.present(), vec!["tate".to_string()]);
    }

    #[test]
    fn incremental_apply_matches_a_full_rebuild() {
        let events = vec![
            ev(1, kind::PRESENCE_ARRIVED, "tate"),
            ev(2, kind::PRESENCE_ARRIVED, "sam"),
            ev(3, kind::PRESENCE_LEFT, "tate"),
            ev(4, kind::PRESENCE_ARRIVED, "marcus"),
        ];

        let mut incremental = Roster::default();
        for e in &events {
            incremental.apply(e);
        }

        assert_eq!(incremental.present(), Roster::rebuild(&events).present());
    }
}
