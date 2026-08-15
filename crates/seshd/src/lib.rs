//! `seshd` — the room daemon. Owns the append-only event log and every
//! view derived from it. Deliberately knows nothing about TVs or phones.

#![warn(missing_docs)]

pub mod event;
