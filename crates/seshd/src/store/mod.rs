//! Persistence. The only place in SESH that writes SQL.
//!
//! Split by responsibility, because the two things stored here have opposite
//! rules and keeping that visible matters more than keeping them in one file:
//!
//! - [`events`] is the **append-only log**. No `UPDATE`, no `DELETE`, ever.
//!   It is the only authoritative state in SESH.
//! - [`people`] is the **identity registry**. It is source data rather than a
//!   projection, so it is the one table that may be updated and migrated.
//!
//! Everything that touches the database lives under this module, so the
//! append-only guarantee can be audited by reading one directory.

mod events;
mod people;

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;

pub use people::Person;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms   INTEGER NOT NULL,
    kind    TEXT    NOT NULL,
    actors  TEXT    NOT NULL,
    subject TEXT,
    payload TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_kind ON events(kind);
CREATE INDEX IF NOT EXISTS idx_events_ts   ON events(ts_ms);

CREATE TABLE IF NOT EXISTS people (
    id     TEXT PRIMARY KEY,
    name   TEXT NOT NULL,
    avatar TEXT
);
"#;

/// The event log and the identity registry, backed by SQLite.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) the database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Open an ephemeral in-memory database. For tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // journal_mode returns the resulting mode, so it must be queried,
        // not executed. In-memory databases report "memory" and that is fine.
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA)?;
        people::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

/// Wall-clock milliseconds since the Unix epoch. The store is the only thing
/// that stamps time, so every row's timestamp comes from one place.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}
