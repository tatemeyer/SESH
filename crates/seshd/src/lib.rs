//! `seshd` — the room daemon. Owns the append-only event log and every
//! view derived from it. Deliberately knows nothing about TVs or phones.

#![warn(missing_docs)]

pub mod api;
pub mod config;
pub mod event;
pub mod launcher;
pub mod projection;
pub mod projections;
pub mod reconcile;
pub mod room;
pub mod store;
