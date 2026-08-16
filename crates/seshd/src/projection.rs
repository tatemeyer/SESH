//! Projections: derived views over the event log.
//!
//! A projection caches state for speed but is never authoritative. Any
//! projection must produce the same result from an incremental stream of
//! events as from a full rebuild — that property is what lets SESH invent
//! new statistics later and apply them to every night ever recorded.

use crate::event::Event;

/// A derived view over the event log.
pub trait Projection: Default {
    /// Fold one event into this view.
    fn apply(&mut self, event: &Event);

    /// Build this view from scratch over an ordered slice of events.
    fn rebuild(events: &[Event]) -> Self
    where
        Self: Sized,
    {
        let mut projection = Self::default();
        for event in events {
            projection.apply(event);
        }
        projection
    }
}
