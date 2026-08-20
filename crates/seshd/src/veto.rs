//! When a veto wins.
//!
//! The rule is "majority veto skips", and the whole question is *majority of
//! whom*. The answer is **presence** — who is in the room — and never
//! *attention*, which is who is looking at their phone this second. They were
//! the same number for as long as the heartbeat was the only signal, and
//! conflating them is what Arc 3 exists to fix: a majority of the people
//! currently staring at a screen is not a majority of the room.
//!
//! `present` here is the log-derived roster from
//! [`Room::roster`](crate::room::Room::roster), which folds `presence.arrived`
//! and `presence.left` from every producer. See
//! [`Presence::attentive`](crate::presence::Presence::attentive) for the other
//! question, and do not pass its result to these functions.
//!
//! Kept as one pure function, deliberately apart from the queue projection.
//! The queue's job is to know what a track's votes are; this decides what they
//! mean, and that is the rule most likely to be argued about on a couch and
//! changed later.

use std::collections::BTreeSet;

/// Fewest votes that can ever skip a track.
///
/// One person in a room of one is not a majority overruling anyone — that is
/// just skipping, and there is a skip button for it. Without this floor a
/// solitary listener's "veto" would be indistinguishable from a vote, and the
/// log would record a unanimous democratic decision every time someone changed
/// their mind alone.
pub const MIN_VOTES: usize = 2;

/// Whether these votes are enough to skip, given who is in the room.
///
/// A strict majority of those present, never fewer than [`MIN_VOTES`].
pub fn should_skip(votes: &BTreeSet<String>, present: &[String]) -> bool {
    counted(votes, present) >= needed(present)
}

/// How many votes a track needs right now.
///
/// Exposed so the phone can render "2/3" rather than making the surface
/// re-derive a rule that lives here.
pub fn needed(present: &[String]) -> usize {
    (present.len() / 2 + 1).max(MIN_VOTES)
}

/// How many of these votes count — those from people still in the room.
///
/// Someone who voted and then left does not get to keep voting from the
/// driveway, but their vote is not an error either: it simply stops counting,
/// and the log keeps the fact that they cast it.
pub fn counted(votes: &BTreeSet<String>, present: &[String]) -> usize {
    votes
        .iter()
        .filter(|voter| present.iter().any(|person| person == *voter))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn votes(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn one_person_alone_cannot_veto() {
        assert!(!should_skip(&votes(&["sam"]), &people(&["sam"])));
    }

    #[test]
    fn both_of_two_can() {
        assert!(should_skip(
            &votes(&["sam", "marcus"]),
            &people(&["sam", "marcus"])
        ));
    }

    #[test]
    fn two_of_three_is_a_majority() {
        let present = people(&["sam", "marcus", "ali"]);
        assert!(should_skip(&votes(&["sam", "marcus"]), &present));
    }

    #[test]
    fn one_of_three_is_not() {
        let present = people(&["sam", "marcus", "ali"]);
        assert!(!should_skip(&votes(&["sam"]), &present));
    }

    #[test]
    fn three_of_five_is_a_majority_and_two_is_not() {
        let present = people(&["a", "b", "c", "d", "e"]);
        assert!(should_skip(&votes(&["a", "b", "c"]), &present));
        assert!(!should_skip(&votes(&["a", "b"]), &present));
    }

    #[test]
    fn two_of_four_is_not_a_strict_majority() {
        let present = people(&["a", "b", "c", "d"]);
        assert!(!should_skip(&votes(&["a", "b"]), &present));
        assert!(should_skip(&votes(&["a", "b", "c"]), &present));
    }

    #[test]
    fn a_vote_from_someone_who_left_does_not_count() {
        let present = people(&["sam", "marcus", "ali"]);
        // Three votes, but one is from a phone that has gone.
        assert!(!should_skip(&votes(&["sam", "ghost"]), &present));
        assert_eq!(counted(&votes(&["sam", "ghost"]), &present), 1);
    }

    #[test]
    fn an_empty_room_cannot_skip_anything() {
        assert!(!should_skip(&votes(&[]), &people(&[])));
        assert!(!should_skip(&votes(&["ghost"]), &people(&[])));
    }

    #[test]
    fn needed_never_drops_below_the_floor() {
        assert_eq!(needed(&people(&[])), MIN_VOTES);
        assert_eq!(needed(&people(&["sam"])), MIN_VOTES);
        assert_eq!(needed(&people(&["sam", "marcus"])), 2);
        assert_eq!(needed(&people(&["a", "b", "c"])), 2);
        assert_eq!(needed(&people(&["a", "b", "c", "d"])), 3);
        assert_eq!(needed(&people(&["a", "b", "c", "d", "e"])), 3);
    }
}
