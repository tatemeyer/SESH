# SESH Arc 1 — The Log & The Room: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Boot the Pi, land on SESH's own screen, and launch and quit Kodi, RetroArch, and Moonlight with a controller — all of it recorded in an append-only event log.

**Architecture:** A Rust daemon (`seshd`) owns an append-only SQLite event log and derives all state from it as projections. It exposes an HTTP + WebSocket API on the LAN and serves a TypeScript surface that Chromium renders fullscreen under labwc. External apps are launched as child processes behind a `Platform` trait, so the launcher is unit-testable off-Pi.

**Tech Stack:** Rust (axum, rusqlite, tokio), TypeScript (Vite, vanilla — no framework), SQLite, systemd user units, labwc, Chromium kiosk.

**Spec:** `docs/superpowers/specs/2026-08-15-sesh-vision-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **Rust edition 2021**, toolchain 1.82 or newer. (Raised from an
  originally arbitrary 1.75 floor: `Cargo.lock` is committed in v4
  format, which requires Cargo 1.78+; 1.82 is the floor actually
  verified against.)
- **Pinned crate versions** (use exactly these majors/minors): `axum = "0.7"`, `tokio = { version = "1", features = ["full"] }`, `rusqlite = { version = "0.31", features = ["bundled"] }`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `toml = "0.8"`, `anyhow = "1"`, `tower-http = { version = "0.5", features = ["fs"] }`, `clap = { version = "4", features = ["derive"] }`, `tracing = "0.1"`, `tracing-subscriber = "0.3"`. Dev-dependencies: `tokio-tungstenite = "0.21"`, `futures-util = "0.3"`, `tempfile = "3"`.
- **axum 0.7 path syntax is `:id`**, not `{id}`. Do not use axum 0.8 syntax.
- `rusqlite`'s `bundled` feature is mandatory — the Pi must not need a system SQLite.
- **The `events` table is append-only.** No `UPDATE` or `DELETE` statement may ever target it, in code or in migrations. There is no API to modify or remove an event.
- **All derived state must be rebuildable from the event log alone.** A projection may cache, but must never be the only copy of a fact.
- **Development happens on Windows; the deploy target is `aarch64-unknown-linux-gnu`.** Every `cargo test` must pass on Windows. Any code that cannot (process spawning, compositor, systemd) goes behind a trait with a mock, or lives in `deploy/` and is verified manually.
- **Per-task gate:** `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` must all be green before the task's commit.
- **Node 20+**, Vite 5, TypeScript 5, Vitest 1. Surface gate: `npm run build` and `npm test` green.
- **Soft ceiling of 300 lines per file.** Split by responsibility when crossed.
- `seshd` binds `0.0.0.0:7373`. No authentication in Arc 1 — the token model arrives in Arc 3.
- Commit messages use Conventional Commits: `type(scope): description`.

---

## File Structure

| Path | Responsibility |
|---|---|
| `Cargo.toml` | Workspace root |
| `crates/seshd/src/lib.rs` | Crate root, module declarations, re-exports |
| `crates/seshd/src/event.rs` | `Event`, `NewEvent`, well-known kind constants |
| `crates/seshd/src/store.rs` | SQLite schema, append, read. The only module that writes SQL |
| `crates/seshd/src/projection.rs` | `Projection` trait and rebuild helper |
| `crates/seshd/src/projections/roster.rs` | Who is present — the first projection |
| `crates/seshd/src/room.rs` | Ties store + broadcast bus + projections together. The service seam |
| `crates/seshd/src/config.rs` | `AppSpec`, `apps.toml` parsing |
| `crates/seshd/src/launcher/platform.rs` | `Platform` trait, `ProcessPlatform`, `MockPlatform` |
| `crates/seshd/src/launcher/mod.rs` | `Launcher`: launch, quit, reap, current |
| `crates/seshd/src/api/mod.rs` | Router assembly, shared `AppState` |
| `crates/seshd/src/api/events.rs` | `GET/POST /api/events`, `GET /api/roster` |
| `crates/seshd/src/api/apps.rs` | `GET /api/apps`, `POST /api/apps/:id/launch`, `POST /api/apps/quit` |
| `crates/seshd/src/api/ws.rs` | `GET /ws` — live event feed |
| `crates/seshd/src/main.rs` | CLI args, wiring, static file serving, serve |
| `surfaces/src/api.ts` | Typed HTTP + WS client |
| `surfaces/src/nav.ts` | Pure grid-navigation logic |
| `surfaces/src/views/home.ts` | The app grid — SESH's front door |
| `surfaces/src/main.ts` | Bootstrap, gamepad/keyboard input loop |
| `deploy/apps.toml` | The app registry |
| `deploy/seshd.service` | systemd **user** unit for `seshd` |
| `deploy/labwc/autostart` | Starts `seshd` and Chromium kiosk inside the Wayland session |
| `deploy/labwc/rc.xml` | labwc config — no decorations |
| `deploy/install.sh` | Pi provisioning |

**Why `seshd` runs as a systemd *user* service:** the launcher spawns Kodi and RetroArch as children, and they need `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` from the compositor's session. Running `seshd` as a system service under a different user would leave spawned apps with no display. Instead, labwc's autostart imports its environment into the user systemd manager and starts `seshd` there, so children inherit the session for free.

---

### Task 1: Workspace scaffold and the event model

**Files:**
- Create: `Cargo.toml`, `crates/seshd/Cargo.toml`, `crates/seshd/src/lib.rs`, `crates/seshd/src/main.rs`, `crates/seshd/src/event.rs`, `.gitignore`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/event.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Event { id: i64, ts_ms: i64, kind: String, actors: Vec<String>, subject: Option<String>, payload: serde_json::Value }`; `NewEvent { kind, actors, subject, payload }` with builder methods `NewEvent::new(kind) -> Self`, `.actor(id) -> Self`, `.subject(s) -> Self`, `.payload(v) -> Self`; module `event::kind` with `PRESENCE_ARRIVED`, `PRESENCE_LEFT`, `APP_LAUNCHED`, `APP_EXITED`.

- [ ] **Step 1: Create the workspace root**

`Cargo.toml`:

```toml
[workspace]
members = ["crates/seshd"]
resolver = "2"
```

`.gitignore`:

```
/target
/surfaces/node_modules
/surfaces/dist
*.db
*.db-wal
*.db-shm
```

- [ ] **Step 2: Create the crate manifest**

`crates/seshd/Cargo.toml`:

```toml
[package]
name = "seshd"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
axum = { version = "0.7", features = ["ws"] }
clap = { version = "4", features = ["derive"] }
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
toml = "0.8"
tower-http = { version = "0.5", features = ["fs"] }
tracing = "0.1"
tracing-subscriber = "0.3"

[dev-dependencies]
futures-util = "0.3"
tempfile = "3"
tokio-tungstenite = "0.21"
```

- [ ] **Step 3: Write the failing test**

`crates/seshd/src/event.rs`:

```rust
//! The event type. Everything in SESH is an event or a view of events.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p seshd`
Expected: FAIL — `cannot find type Event`, `cannot find type NewEvent`.

- [ ] **Step 5: Write the implementation**

Prepend to `crates/seshd/src/event.rs`, above the test module:

```rust
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
}
```

`crates/seshd/src/lib.rs`:

```rust
//! `seshd` — the room daemon. Owns the append-only event log and every
//! view derived from it. Deliberately knows nothing about TVs or phones.

#![warn(missing_docs)]

pub mod event;
```

`crates/seshd/src/main.rs`:

```rust
//! Binary entry point. Wiring only — see `lib.rs` for the daemon itself.

fn main() {
    println!("seshd");
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p seshd`
Expected: PASS — 3 tests.

- [ ] **Step 7: Run the gate**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: no output, exit 0.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml .gitignore crates/
git commit -m "feat(core): add workspace scaffold and the event model

Event kind is a free-form string rather than an enum so future producers
can add kinds without a schema migration, which the deferred game-capture
decision depends on."
```

---

### Task 2: The append-only event store

**Files:**
- Create: `crates/seshd/src/store.rs`
- Modify: `crates/seshd/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/store.rs`

**Interfaces:**
- Consumes: `event::{Event, NewEvent}` from Task 1.
- Produces: `Store::open(path: &Path) -> anyhow::Result<Store>`, `Store::open_in_memory() -> anyhow::Result<Store>`, `Store::append(&self, new: NewEvent) -> anyhow::Result<Event>`, `Store::read_since(&self, after_id: i64, limit: i64) -> anyhow::Result<Vec<Event>>` (pass `limit = -1` for unlimited), `Store::last_id(&self) -> anyhow::Result<i64>`.

- [ ] **Step 1: Write the failing test**

`crates/seshd/src/store.rs`:

```rust
//! The append-only event log. The only module in SESH that writes SQL.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;

use crate::event::{Event, NewEvent};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_increasing_ids_and_a_timestamp() {
        let store = Store::open_in_memory().unwrap();
        let a = store.append(NewEvent::new("a")).unwrap();
        let b = store.append(NewEvent::new("b")).unwrap();

        assert!(b.id > a.id, "ids must increase: {} then {}", a.id, b.id);
        assert!(a.ts_ms > 0, "timestamp must be set");
    }

    #[test]
    fn append_preserves_every_field() {
        let store = Store::open_in_memory().unwrap();
        let written = store
            .append(
                NewEvent::new("match.result")
                    .actor("tate")
                    .actor("sam")
                    .subject("mario-kart")
                    .payload(serde_json::json!({ "track": "rainbow-road" })),
            )
            .unwrap();

        let read = store.read_since(0, -1).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], written);
        assert_eq!(read[0].actors, vec!["tate".to_string(), "sam".to_string()]);
        assert_eq!(read[0].payload["track"], "rainbow-road");
    }

    #[test]
    fn read_since_returns_only_later_events_in_order() {
        let store = Store::open_in_memory().unwrap();
        let first = store.append(NewEvent::new("a")).unwrap();
        store.append(NewEvent::new("b")).unwrap();
        store.append(NewEvent::new("c")).unwrap();

        let rest = store.read_since(first.id, -1).unwrap();
        let kinds: Vec<_> = rest.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["b", "c"]);
    }

    #[test]
    fn read_since_respects_the_limit() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..5 {
            store.append(NewEvent::new(format!("k{i}"))).unwrap();
        }
        assert_eq!(store.read_since(0, 2).unwrap().len(), 2);
    }

    #[test]
    fn events_survive_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        {
            let store = Store::open(&path).unwrap();
            store.append(NewEvent::new("presence.arrived").actor("sam")).unwrap();
        }

        let reopened = Store::open(&path).unwrap();
        let events = reopened.read_since(0, -1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actors, vec!["sam".to_string()]);
    }

    #[test]
    fn last_id_is_zero_on_an_empty_log() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.last_id().unwrap(), 0);
        let e = store.append(NewEvent::new("a")).unwrap();
        assert_eq!(store.last_id().unwrap(), e.id);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd store`
Expected: FAIL — `cannot find type Store`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/seshd/src/store.rs`, between the `use` block and the test module:

```rust
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

/// The append-only event log, backed by SQLite.
///
/// There is deliberately no update or delete operation. The log is the only
/// authoritative state in SESH; everything else is derived from it.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) the log at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Open an ephemeral in-memory log. For tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // journal_mode returns the resulting mode, so it must be queried,
        // not executed. In-memory databases report "memory" and that is fine.
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Append an event and return it with its assigned id and timestamp.
    pub fn append(&self, new: NewEvent) -> Result<Event> {
        let ts_ms = now_ms();
        let actors = serde_json::to_string(&new.actors)?;
        let payload = serde_json::to_string(&new.payload)?;

        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO events (ts_ms, kind, actors, subject, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![ts_ms, new.kind, actors, new.subject, payload],
        )?;
        let id = conn.last_insert_rowid();

        Ok(Event {
            id,
            ts_ms,
            kind: new.kind,
            actors: new.actors,
            subject: new.subject,
            payload: new.payload,
        })
    }

    /// Read events with an id greater than `after_id`, oldest first.
    /// Pass `limit = -1` for no limit.
    pub fn read_since(&self, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, ts_ms, kind, actors, subject, payload
             FROM events WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![after_id, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (id, ts_ms, kind, actors, subject, payload) = row?;
            out.push(Event {
                id,
                ts_ms,
                kind,
                actors: serde_json::from_str(&actors)?,
                subject,
                payload: serde_json::from_str(&payload)?,
            });
        }
        Ok(out)
    }

    /// The highest id in the log, or 0 if it is empty.
    pub fn last_id(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?;
        Ok(id)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}
```

Add to `crates/seshd/src/lib.rs`:

```rust
pub mod store;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p seshd store`
Expected: PASS — 6 tests.

- [ ] **Step 5: Verify the append-only constraint holds**

Run: `grep -rniE "(UPDATE|DELETE)[[:space:]]+" crates/seshd/src/store.rs`
Expected: no matches. If this ever matches, the core invariant of the system has been broken.

- [ ] **Step 6: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add crates/seshd/src/store.rs crates/seshd/src/lib.rs
git commit -m "feat(core): add the append-only event store

SQLite with WAL and synchronous=FULL. No update or delete path exists by
design: the log is the only authoritative state and everything else in
SESH is rebuilt from it."
```

---

### Task 3: Projections — the trait and the roster

**Files:**
- Create: `crates/seshd/src/projection.rs`, `crates/seshd/src/projections/mod.rs`, `crates/seshd/src/projections/roster.rs`
- Modify: `crates/seshd/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/projections/roster.rs`

**Interfaces:**
- Consumes: `event::{Event, kind}` from Task 1.
- Produces: `trait Projection: Default { fn apply(&mut self, event: &Event); fn rebuild(events: &[Event]) -> Self where Self: Sized; }`; `struct Roster` implementing `Projection`, with `Roster::present(&self) -> Vec<String>`.

- [ ] **Step 1: Write the failing test**

`crates/seshd/src/projections/roster.rs`:

```rust
//! Who is in the room right now. The first projection — the shape every
//! later one (trophy case, brackets, streaks) follows.

use std::collections::BTreeSet;

use crate::event::{kind, Event};
use crate::projection::Projection;

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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd roster`
Expected: FAIL — `cannot find trait Projection`, `cannot find type Roster`.

- [ ] **Step 3: Write the trait**

`crates/seshd/src/projection.rs`:

```rust
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
```

- [ ] **Step 4: Write the roster**

Insert into `crates/seshd/src/projections/roster.rs`, between the `use` block and the test module:

```rust
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
```

`crates/seshd/src/projections/mod.rs`:

```rust
//! Derived views over the event log.

pub mod roster;
```

Add to `crates/seshd/src/lib.rs`:

```rust
pub mod projection;
pub mod projections;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p seshd roster`
Expected: PASS — 6 tests.

- [ ] **Step 6: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add crates/seshd/src/projection.rs crates/seshd/src/projections crates/seshd/src/lib.rs
git commit -m "feat(core): add the projection trait and the roster view

Roster is deliberately the simplest possible projection: it establishes
the incremental-equals-rebuild property that the trophy case and brackets
will rely on in Arc 4."
```

---

### Task 4: The Room — store, bus, and projections wired together

**Files:**
- Create: `crates/seshd/src/room.rs`
- Modify: `crates/seshd/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/room.rs`

**Interfaces:**
- Consumes: `Store` (Task 2), `Roster` + `Projection` (Task 3), `event::{Event, NewEvent}` (Task 1).
- Produces: `Room::new(store: Store) -> anyhow::Result<Arc<Room>>`, `Room::record(&self, new: NewEvent) -> anyhow::Result<Event>`, `Room::subscribe(&self) -> tokio::sync::broadcast::Receiver<Event>`, `Room::roster(&self) -> Vec<String>`, `Room::events_since(&self, after_id: i64, limit: i64) -> anyhow::Result<Vec<Event>>`.

- [ ] **Step 1: Write the failing test**

`crates/seshd/src/room.rs`:

```rust
//! The Room: the seam between the event log and everything that reads it.
//!
//! Every write to SESH goes through `Room::record`, which appends to the
//! log, folds the event into live projections, and fans it out to
//! subscribers. Nothing else may write to the store.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::broadcast;

use crate::event::{Event, NewEvent};
use crate::projection::Projection;
use crate::projections::roster::Roster;
use crate::store::Store;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::kind;

    fn room() -> Arc<Room> {
        Room::new(Store::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn recording_persists_the_event() {
        let room = room();
        room.record(NewEvent::new("a")).unwrap();
        room.record(NewEvent::new("b")).unwrap();

        let events = room.events_since(0, -1).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn recording_updates_the_roster() {
        let room = room();
        assert!(room.roster().is_empty());

        room.record(NewEvent::new(kind::PRESENCE_ARRIVED).actor("tate")).unwrap();
        assert_eq!(room.roster(), vec!["tate".to_string()]);

        room.record(NewEvent::new(kind::PRESENCE_LEFT).actor("tate")).unwrap();
        assert!(room.roster().is_empty());
    }

    #[tokio::test]
    async fn subscribers_receive_recorded_events() {
        let room = room();
        let mut rx = room.subscribe();

        let written = room.record(NewEvent::new("moment.captured").subject("clip-1")).unwrap();
        let received = rx.recv().await.unwrap();

        assert_eq!(received, written);
    }

    #[test]
    fn projections_are_restored_from_the_log_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        {
            let room = Room::new(Store::open(&path).unwrap()).unwrap();
            room.record(NewEvent::new(kind::PRESENCE_ARRIVED).actor("sam")).unwrap();
        }

        let reopened = Room::new(Store::open(&path).unwrap()).unwrap();
        assert_eq!(reopened.roster(), vec!["sam".to_string()]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd room`
Expected: FAIL — `cannot find type Room`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/seshd/src/room.rs`, between the `use` block and the test module:

```rust
/// Broadcast backlog. A subscriber that falls this far behind is lagged,
/// and reconnects with `GET /api/events?after=<last_id>` to catch up.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// The live room: the event log plus every view derived from it.
pub struct Room {
    store: Store,
    events_tx: broadcast::Sender<Event>,
    roster: Mutex<Roster>,
}

impl Room {
    /// Open a room over `store`, restoring projections from the log.
    pub fn new(store: Store) -> Result<Arc<Self>> {
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let history = store.read_since(0, -1)?;
        let roster = Roster::rebuild(&history);

        Ok(Arc::new(Self {
            store,
            events_tx,
            roster: Mutex::new(roster),
        }))
    }

    /// Append an event, update projections, and fan it out. The only write path.
    pub fn record(&self, new: NewEvent) -> Result<Event> {
        let event = self.store.append(new)?;
        self.roster
            .lock()
            .expect("roster mutex poisoned")
            .apply(&event);
        // Fails only when there are no subscribers, which is not an error.
        let _ = self.events_tx.send(event.clone());
        Ok(event)
    }

    /// Subscribe to the live event feed.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    /// Person ids currently in the room.
    pub fn roster(&self) -> Vec<String> {
        self.roster
            .lock()
            .expect("roster mutex poisoned")
            .present()
    }

    /// Read history. Pass `limit = -1` for no limit.
    pub fn events_since(&self, after_id: i64, limit: i64) -> Result<Vec<Event>> {
        self.store.read_since(after_id, limit)
    }
}
```

Add to `crates/seshd/src/lib.rs`:

```rust
pub mod room;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p seshd room`
Expected: PASS — 4 tests.

- [ ] **Step 5: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/seshd/src/room.rs crates/seshd/src/lib.rs
git commit -m "feat(core): add Room, the single write path into the log

Room::record is the only way anything enters SESH: it appends, folds the
event into live projections, and fans out to subscribers, so no caller can
update state without leaving a record."
```

---

### Task 5: The Platform trait and its two implementations

**Files:**
- Create: `crates/seshd/src/launcher/platform.rs`, `crates/seshd/src/launcher/mod.rs`
- Modify: `crates/seshd/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/launcher/platform.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `type Pid = u32`; `trait Platform: Send + Sync + 'static { fn spawn(&self, program: &str, args: &[String]) -> anyhow::Result<Pid>; fn kill(&self, pid: Pid) -> anyhow::Result<()>; fn is_running(&self, pid: Pid) -> bool; }`; `ProcessPlatform::new() -> ProcessPlatform`; `MockPlatform::new() -> MockPlatform` with `MockPlatform::spawned(&self) -> Vec<(String, Vec<String>)>` and `MockPlatform::simulate_exit(&self, pid: Pid)`.

**Why this exists:** development happens on Windows and the target is a Pi. Every consumer of process control talks to this trait, so the launcher's logic is fully testable off-target and only `ProcessPlatform` needs real hardware.

- [ ] **Step 1: Write the failing test**

`crates/seshd/src/launcher/platform.rs`:

```rust
//! Process control, behind a trait so the launcher is testable off-Pi.

use std::collections::{HashMap, HashSet};
use std::process::{Child, Command};
use std::sync::Mutex;

use anyhow::{anyhow, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_records_what_was_spawned() {
        let platform = MockPlatform::new();
        platform.spawn("kodi", &["--fullscreen".to_string()]).unwrap();

        assert_eq!(
            platform.spawned(),
            vec![("kodi".to_string(), vec!["--fullscreen".to_string()])]
        );
    }

    #[test]
    fn mock_reports_spawned_processes_as_running() {
        let platform = MockPlatform::new();
        let pid = platform.spawn("retroarch", &[]).unwrap();
        assert!(platform.is_running(pid));
    }

    #[test]
    fn mock_kill_stops_a_process() {
        let platform = MockPlatform::new();
        let pid = platform.spawn("retroarch", &[]).unwrap();
        platform.kill(pid).unwrap();
        assert!(!platform.is_running(pid));
    }

    #[test]
    fn mock_can_simulate_a_process_exiting_on_its_own() {
        let platform = MockPlatform::new();
        let pid = platform.spawn("kodi", &[]).unwrap();
        platform.simulate_exit(pid);
        assert!(!platform.is_running(pid));
    }

    #[test]
    fn mock_pids_are_unique() {
        let platform = MockPlatform::new();
        let a = platform.spawn("a", &[]).unwrap();
        let b = platform.spawn("b", &[]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn process_platform_spawns_and_kills_a_real_process() {
        let platform = ProcessPlatform::new();

        // A long-running process that needs no console and no stdin.
        // `timeout` is not usable here: it fails without a real console.
        #[cfg(windows)]
        let (program, args) = (
            "ping",
            vec!["-n".to_string(), "60".to_string(), "127.0.0.1".to_string()],
        );
        #[cfg(unix)]
        let (program, args) = ("sleep", vec!["60".to_string()]);

        let pid = platform.spawn(program, &args).unwrap();
        assert!(platform.is_running(pid));

        platform.kill(pid).unwrap();
        assert!(!platform.is_running(pid));
    }

    #[test]
    fn process_platform_errors_on_a_missing_program() {
        let platform = ProcessPlatform::new();
        assert!(platform.spawn("definitely-not-a-real-program-xyz", &[]).is_err());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd platform`
Expected: FAIL — `cannot find type MockPlatform`, `cannot find type ProcessPlatform`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/seshd/src/launcher/platform.rs`, between the `use` block and the test module:

```rust
/// A process handle. For `ProcessPlatform` this is the OS pid.
pub type Pid = u32;

/// Everything SESH needs from the operating system to run an app.
pub trait Platform: Send + Sync + 'static {
    /// Start a program and return a handle to it.
    fn spawn(&self, program: &str, args: &[String]) -> Result<Pid>;

    /// Stop a process. Stopping an already-dead process is not an error.
    fn kill(&self, pid: Pid) -> Result<()>;

    /// Whether the process is still alive.
    fn is_running(&self, pid: Pid) -> bool;
}

/// The real implementation: spawns child processes of `seshd`.
///
/// Because `seshd` runs inside the compositor's user session, children
/// inherit `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` and appear on the TV.
#[derive(Default)]
pub struct ProcessPlatform {
    children: Mutex<HashMap<Pid, Child>>,
}

impl ProcessPlatform {
    /// Create a platform backed by real OS processes.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Platform for ProcessPlatform {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Pid> {
        let child = Command::new(program).args(args).spawn()?;
        let pid = child.id();
        self.children
            .lock()
            .expect("children mutex poisoned")
            .insert(pid, child);
        Ok(pid)
    }

    fn kill(&self, pid: Pid) -> Result<()> {
        let mut children = self.children.lock().expect("children mutex poisoned");
        if let Some(child) = children.get_mut(&pid) {
            // An already-exited child is the normal case when the user quit
            // the app themselves, so a failed kill is not an error.
            let _ = child.kill();
            let _ = child.wait();
            children.remove(&pid);
        }
        Ok(())
    }

    fn is_running(&self, pid: Pid) -> bool {
        let mut children = self.children.lock().expect("children mutex poisoned");
        let running = matches!(children.get_mut(&pid).map(|c| c.try_wait()), Some(Ok(None)));
        if !running {
            // A process that exited on its own (the "quit Kodi from its own
            // menu" case) never has kill() called on it, so this is the
            // only place a dead child's entry — and on Windows its open
            // process HANDLE — ever gets reclaimed.
            children.remove(&pid);
        }
        running
    }
}

/// An in-memory platform for tests. Records every spawn and lets a test
/// simulate an app the user quit from inside itself.
#[derive(Default)]
pub struct MockPlatform {
    next_pid: Mutex<Pid>,
    running: Mutex<HashSet<Pid>>,
    spawned: Mutex<Vec<(String, Vec<String>)>>,
}

impl MockPlatform {
    /// Create an empty mock platform.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `(program, args)` pair spawned so far, in order.
    pub fn spawned(&self) -> Vec<(String, Vec<String>)> {
        self.spawned.lock().expect("spawned mutex poisoned").clone()
    }

    /// Mark a process as having exited on its own, without SESH killing it.
    pub fn simulate_exit(&self, pid: Pid) {
        self.running.lock().expect("running mutex poisoned").remove(&pid);
    }
}

impl Platform for MockPlatform {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Pid> {
        if program.is_empty() {
            return Err(anyhow!("empty program name"));
        }
        let mut next = self.next_pid.lock().expect("next_pid mutex poisoned");
        *next += 1;
        let pid = *next;
        self.running.lock().expect("running mutex poisoned").insert(pid);
        self.spawned
            .lock()
            .expect("spawned mutex poisoned")
            .push((program.to_string(), args.to_vec()));
        Ok(pid)
    }

    fn kill(&self, pid: Pid) -> Result<()> {
        self.running.lock().expect("running mutex poisoned").remove(&pid);
        Ok(())
    }

    fn is_running(&self, pid: Pid) -> bool {
        self.running.lock().expect("running mutex poisoned").contains(&pid)
    }
}
```

`crates/seshd/src/launcher/mod.rs`:

```rust
//! Starting, stopping, and reaping the apps SESH launches.

pub mod platform;
```

Add to `crates/seshd/src/lib.rs`:

```rust
pub mod launcher;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p seshd platform`
Expected: PASS — 7 tests. The two `ProcessPlatform` tests run real processes and pass on both Windows and Linux.

- [ ] **Step 5: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/seshd/src/launcher crates/seshd/src/lib.rs
git commit -m "feat(launcher): add the Platform trait with process and mock impls

Development happens on Windows and the target is a Pi, so all process
control goes behind a trait. MockPlatform can simulate an app the user
quit from inside itself, which the reaper needs to be testable."
```

---

### Task 6: The app registry

**Files:**
- Create: `crates/seshd/src/config.rs`, `deploy/apps.toml`
- Modify: `crates/seshd/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `struct AppSpec { pub id: String, pub name: String, pub command: String, pub args: Vec<String>, pub icon: String }` (derives `Debug, Clone, PartialEq, Serialize, Deserialize`); `load_apps(toml_str: &str) -> anyhow::Result<Vec<AppSpec>>`; `load_apps_file(path: &std::path::Path) -> anyhow::Result<Vec<AppSpec>>`.

- [ ] **Step 1: Write the failing test**

`crates/seshd/src/config.rs`:

```rust
//! The app registry: what SESH can launch, read from `apps.toml`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_app_entry() {
        let apps = load_apps(
            r#"
[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"
args = ["--standalone"]
icon = "movie"
"#,
        )
        .unwrap();

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "kodi");
        assert_eq!(apps[0].name, "Kodi");
        assert_eq!(apps[0].command, "kodi");
        assert_eq!(apps[0].args, vec!["--standalone".to_string()]);
        assert_eq!(apps[0].icon, "movie");
    }

    #[test]
    fn args_and_icon_are_optional() {
        let apps = load_apps(
            r#"
[[app]]
id = "retroarch"
name = "RetroArch"
command = "retroarch"
"#,
        )
        .unwrap();

        assert!(apps[0].args.is_empty());
        assert_eq!(apps[0].icon, "");
    }

    #[test]
    fn parses_multiple_apps_in_order() {
        let apps = load_apps(
            r#"
[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"

[[app]]
id = "moonlight"
name = "Moonlight"
command = "moonlight"
"#,
        )
        .unwrap();

        let ids: Vec<_> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["kodi", "moonlight"]);
    }

    #[test]
    fn an_empty_registry_is_valid() {
        assert!(load_apps("").unwrap().is_empty());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let err = load_apps(
            r#"
[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"

[[app]]
id = "kodi"
name = "Kodi Again"
command = "kodi"
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("kodi"), "error should name the id: {err}");
    }

    #[test]
    fn malformed_toml_is_rejected() {
        assert!(load_apps("this is not toml [[[").is_err());
    }

    #[test]
    fn the_shipped_registry_parses() {
        let toml = std::fs::read_to_string("../../deploy/apps.toml").unwrap();
        let apps = load_apps(&toml).unwrap();
        let ids: Vec<_> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["kodi", "retroarch", "moonlight"]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd config`
Expected: FAIL — `cannot find function load_apps`.

- [ ] **Step 3: Write the registry file**

`deploy/apps.toml`:

```toml
# What SESH can launch. `command` must be on PATH inside the compositor
# session. Verify each with `which <command>` on the Pi before trusting it.

[[app]]
id = "kodi"
name = "Kodi"
command = "kodi"
args = ["--standalone"]
icon = "movie"

[[app]]
id = "retroarch"
name = "RetroArch"
command = "retroarch"
args = ["--fullscreen"]
icon = "gamepad"

# Replace GAMING-PC with the hostname or LAN IP of the machine running
# Sunshine, and Desktop with the app name Sunshine exposes.
[[app]]
id = "moonlight"
name = "Moonlight"
command = "moonlight"
args = ["stream", "GAMING-PC", "Desktop"]
icon = "display"
```

- [ ] **Step 4: Write the implementation**

Insert into `crates/seshd/src/config.rs`, between the `use` block and the test module:

```rust
/// One launchable app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSpec {
    /// Stable identifier used in URLs and event subjects.
    pub id: String,
    /// Display name shown on the TV.
    pub name: String,
    /// Program to execute. Must be on PATH inside the compositor session.
    pub command: String,
    /// Arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Icon name the surface renders. Free-form; the surface decides.
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Deserialize)]
struct AppsFile {
    #[serde(default)]
    app: Vec<AppSpec>,
}

/// Parse an app registry from TOML.
pub fn load_apps(toml_str: &str) -> Result<Vec<AppSpec>> {
    let parsed: AppsFile = toml::from_str(toml_str).context("apps registry is not valid TOML")?;

    let mut seen = std::collections::HashSet::new();
    for app in &parsed.app {
        if !seen.insert(app.id.as_str()) {
            anyhow::bail!("duplicate app id in registry: {}", app.id);
        }
    }

    Ok(parsed.app)
}

/// Read and parse an app registry from disk.
pub fn load_apps_file(path: &Path) -> Result<Vec<AppSpec>> {
    let toml_str = std::fs::read_to_string(path)
        .with_context(|| format!("reading app registry {}", path.display()))?;
    load_apps(&toml_str)
}
```

Add to `crates/seshd/src/lib.rs`:

```rust
pub mod config;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p seshd config`
Expected: PASS — 7 tests.

- [ ] **Step 6: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add crates/seshd/src/config.rs crates/seshd/src/lib.rs deploy/apps.toml
git commit -m "feat(launcher): add the app registry

Apps are configuration rather than code so a new emulator or streaming
target is a TOML edit on the Pi, not a rebuild."
```

---

### Task 7: The Launcher, including the reaper

**Files:**
- Modify: `crates/seshd/src/launcher/mod.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/launcher/mod.rs`

**Interfaces:**
- Consumes: `Platform`, `Pid`, `MockPlatform` (Task 5); `AppSpec` (Task 6); `Room`, `NewEvent`, `event::kind` (Tasks 1 and 4).
- Produces: `Launcher::new(apps: Vec<AppSpec>, platform: Arc<dyn Platform>, room: Arc<Room>) -> Arc<Launcher>`, `Launcher::apps(&self) -> &[AppSpec]`, `Launcher::current(&self) -> Option<String>`, `Launcher::launch(&self, id: &str) -> anyhow::Result<()>`, `Launcher::quit(&self) -> anyhow::Result<()>`, `Launcher::reap(&self) -> anyhow::Result<()>`, `Launcher::reap_loop(launcher: Arc<Launcher>)` (async, never returns).

**Why the reaper exists:** if you quit Kodi from Kodi's own menu, nothing tells `seshd`. Without a reaper, SESH believes Kodi is still running forever and the home screen stays wrong.

- [ ] **Step 1: Write the failing test**

Replace the contents of `crates/seshd/src/launcher/mod.rs`:

```rust
//! Starting, stopping, and reaping the apps SESH launches.
//!
//! Exactly one app runs at a time: launching while something is running
//! quits it first. The compositor stacks the new window over the SESH
//! kiosk, and killing it reveals SESH again, so there is no focus
//! management to do here.

pub mod platform;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::config::AppSpec;
use crate::event::{kind, NewEvent};
use crate::room::Room;
use platform::{Pid, Platform};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use platform::MockPlatform;

    fn fixture() -> (Arc<Launcher>, Arc<MockPlatform>, Arc<Room>) {
        let apps = vec![
            AppSpec {
                id: "kodi".into(),
                name: "Kodi".into(),
                command: "kodi".into(),
                args: vec!["--standalone".into()],
                icon: "movie".into(),
            },
            AppSpec {
                id: "retroarch".into(),
                name: "RetroArch".into(),
                command: "retroarch".into(),
                args: vec![],
                icon: "gamepad".into(),
            },
        ];
        let platform = Arc::new(MockPlatform::new());
        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let launcher = Launcher::new(apps, platform.clone(), room.clone());
        (launcher, platform, room)
    }

    fn kinds(room: &Room) -> Vec<String> {
        room.events_since(0, -1)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    fn subjects(room: &Room) -> Vec<Option<String>> {
        room.events_since(0, -1)
            .unwrap()
            .into_iter()
            .map(|e| e.subject)
            .collect()
    }

    #[test]
    fn nothing_is_running_initially() {
        let (launcher, _, _) = fixture();
        assert_eq!(launcher.current(), None);
    }

    #[test]
    fn launching_starts_the_configured_command_with_its_args() {
        let (launcher, platform, _) = fixture();
        launcher.launch("kodi").unwrap();

        assert_eq!(
            platform.spawned(),
            vec![("kodi".to_string(), vec!["--standalone".to_string()])]
        );
        assert_eq!(launcher.current(), Some("kodi".to_string()));
    }

    #[test]
    fn launching_records_an_app_launched_event() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();

        assert_eq!(kinds(&room), vec![kind::APP_LAUNCHED.to_string()]);
        assert_eq!(subjects(&room), vec![Some("kodi".to_string())]);
    }

    #[test]
    fn launching_an_unknown_app_is_an_error_and_records_nothing() {
        let (launcher, _, room) = fixture();
        let err = launcher.launch("nintendo64").unwrap_err();

        assert!(err.to_string().contains("nintendo64"), "error should name the id: {err}");
        assert!(kinds(&room).is_empty());
        assert_eq!(launcher.current(), None);
    }

    #[test]
    fn launching_while_running_quits_the_previous_app_first() {
        let (launcher, platform, room) = fixture();
        launcher.launch("kodi").unwrap();
        launcher.launch("retroarch").unwrap();

        assert_eq!(launcher.current(), Some("retroarch".to_string()));
        assert_eq!(
            kinds(&room),
            vec![
                kind::APP_LAUNCHED.to_string(),
                kind::APP_EXITED.to_string(),
                kind::APP_LAUNCHED.to_string(),
            ]
        );
        assert_eq!(platform.spawned().len(), 2);
    }

    #[test]
    fn quitting_stops_the_app_and_records_an_exit() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();
        launcher.quit().unwrap();

        assert_eq!(launcher.current(), None);
        assert_eq!(
            kinds(&room),
            vec![kind::APP_LAUNCHED.to_string(), kind::APP_EXITED.to_string()]
        );
    }

    #[test]
    fn quitting_with_nothing_running_is_a_no_op() {
        let (launcher, _, room) = fixture();
        launcher.quit().unwrap();

        assert!(kinds(&room).is_empty());
        assert_eq!(launcher.current(), None);
    }

    #[test]
    fn reaping_notices_an_app_the_user_quit_from_inside_itself() {
        let (launcher, platform, room) = fixture();
        launcher.launch("kodi").unwrap();

        // The user picked Kodi's own Quit menu item. SESH did not do this.
        let pid = launcher.current_pid().unwrap();
        platform.simulate_exit(pid);

        launcher.reap().unwrap();

        assert_eq!(launcher.current(), None);
        assert_eq!(
            kinds(&room),
            vec![kind::APP_LAUNCHED.to_string(), kind::APP_EXITED.to_string()]
        );
    }

    #[test]
    fn reaping_a_still_running_app_changes_nothing() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();
        launcher.reap().unwrap();

        assert_eq!(launcher.current(), Some("kodi".to_string()));
        assert_eq!(kinds(&room), vec![kind::APP_LAUNCHED.to_string()]);
    }

    #[test]
    fn reaping_with_nothing_running_is_a_no_op() {
        let (launcher, _, room) = fixture();
        launcher.reap().unwrap();
        assert!(kinds(&room).is_empty());
    }

    #[test]
    fn apps_are_exposed_in_registry_order() {
        let (launcher, _, _) = fixture();
        let ids: Vec<_> = launcher.apps().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["kodi", "retroarch"]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd launcher`
Expected: FAIL — `cannot find type Launcher`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/seshd/src/launcher/mod.rs`, between the `use` block and the test module:

```rust
/// How often the reaper checks whether the current app is still alive.
const REAP_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
struct Running {
    app_id: String,
    pid: Pid,
}

/// Runs at most one app at a time and keeps the log in sync with reality.
pub struct Launcher {
    apps: Vec<AppSpec>,
    platform: Arc<dyn Platform>,
    room: Arc<Room>,
    current: Mutex<Option<Running>>,
}

impl Launcher {
    /// Build a launcher over an app registry.
    pub fn new(apps: Vec<AppSpec>, platform: Arc<dyn Platform>, room: Arc<Room>) -> Arc<Self> {
        Arc::new(Self {
            apps,
            platform,
            room,
            current: Mutex::new(None),
        })
    }

    /// Every launchable app, in registry order.
    pub fn apps(&self) -> &[AppSpec] {
        &self.apps
    }

    /// The id of the app currently running, if any.
    pub fn current(&self) -> Option<String> {
        self.current
            .lock()
            .expect("current mutex poisoned")
            .as_ref()
            .map(|r| r.app_id.clone())
    }

    /// The pid of the app currently running, if any. Used by tests and the reaper.
    pub fn current_pid(&self) -> Option<Pid> {
        self.current
            .lock()
            .expect("current mutex poisoned")
            .as_ref()
            .map(|r| r.pid)
    }

    /// Quit whatever is running, then start `id`.
    pub fn launch(&self, id: &str) -> Result<()> {
        let spec = self
            .apps
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| anyhow!("no such app: {id}"))?
            .clone();

        self.quit()?;

        let pid = self.platform.spawn(&spec.command, &spec.args)?;
        // The log write is the commit point. If it fails, undo the spawn rather
        // than leaving a running process SESH has no record of and cannot kill.
        if let Err(error) = self
            .room
            .record(NewEvent::new(kind::APP_LAUNCHED).subject(&spec.id))
        {
            let _ = self.platform.kill(pid);
            return Err(error);
        }
        *self.current.lock().expect("current mutex poisoned") = Some(Running {
            app_id: spec.id.clone(),
            pid,
        });
        Ok(())
    }

    /// Stop the running app, if any.
    pub fn quit(&self) -> Result<()> {
        let mut current = self.current.lock().expect("current mutex poisoned");
        let Some(running) = current.as_ref() else {
            return Ok(());
        };
        let (pid, app_id) = (running.pid, running.app_id.clone());
        self.platform.kill(pid)?;
        self.room
            .record(NewEvent::new(kind::APP_EXITED).subject(&app_id))?;
        *current = None;
        Ok(())
    }

    /// Notice an app that exited on its own — the user quit it from inside
    /// itself, or it crashed — and record the exit.
    pub fn reap(&self) -> Result<()> {
        let mut current = self.current.lock().expect("current mutex poisoned");
        let Some(running) = current.as_ref() else {
            return Ok(());
        };
        if self.platform.is_running(running.pid) {
            return Ok(());
        }
        let app_id = running.app_id.clone();
        // Record before forgetting: if the log write fails, `current` stays set
        // and the next reap retries, rather than silently losing the exit.
        self.room
            .record(NewEvent::new(kind::APP_EXITED).subject(&app_id))?;
        *current = None;
        Ok(())
    }

    /// Reap forever. Spawned as a background task by `main`.
    pub async fn reap_loop(launcher: Arc<Self>) {
        loop {
            tokio::time::sleep(REAP_INTERVAL).await;
            if let Err(error) = launcher.reap() {
                tracing::warn!(%error, "reaper failed");
            }
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p seshd launcher`
Expected: PASS — 11 launcher tests plus the 7 platform tests.

- [ ] **Step 5: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/seshd/src/launcher/mod.rs
git commit -m "feat(launcher): run one app at a time and reap self-quit apps

Quitting Kodi from Kodi's own menu tells SESH nothing, so a reaper polls
process liveness and records app.exited. Without it the home screen shows
a running app forever."
```

---

### Task 8: The HTTP API

**Files:**
- Create: `crates/seshd/src/api/mod.rs`, `crates/seshd/src/api/events.rs`, `crates/seshd/src/api/apps.rs`
- Modify: `crates/seshd/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/seshd/src/api/mod.rs`

**Interfaces:**
- Consumes: `Room` (Task 4), `Launcher` (Task 7), `AppSpec` (Task 6), `Event`/`NewEvent` (Task 1).
- Produces: `struct AppState { pub room: Arc<Room>, pub launcher: Arc<Launcher> }` (derives `Clone`); `api::router(state: AppState) -> axum::Router`.

**Routes:**

| Method | Path | Body | Response |
|---|---|---|---|
| `GET` | `/api/events?after=<id>&limit=<n>` | — | `200` `Event[]` |
| `POST` | `/api/events` | `NewEvent` | `201` `Event` |
| `GET` | `/api/roster` | — | `200` `string[]` |
| `GET` | `/api/apps` | — | `200` `{ apps: AppSpec[], current: string \| null }` |
| `POST` | `/api/apps/:id/launch` | — | `204`, or `404` if unknown |
| `POST` | `/api/apps/quit` | — | `204` |

- [ ] **Step 1: Write the failing test**

`crates/seshd/src/api/mod.rs`:

```rust
//! The LAN-facing HTTP and WebSocket API. Arc 1 is unauthenticated by
//! design; the per-person token model arrives with phones in Arc 3.

pub mod apps;
pub mod events;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::launcher::Launcher;
use crate::room::Room;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSpec;
    use crate::launcher::platform::MockPlatform;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> (Router, Arc<Room>, Arc<Launcher>) {
        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let launcher = Launcher::new(
            vec![AppSpec {
                id: "kodi".into(),
                name: "Kodi".into(),
                command: "kodi".into(),
                args: vec![],
                icon: "movie".into(),
            }],
            Arc::new(MockPlatform::new()),
            room.clone(),
        );
        let state = AppState {
            room: room.clone(),
            launcher: launcher.clone(),
        };
        (router(state), room, launcher)
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn post_empty(uri: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_events_is_empty_on_a_fresh_log() {
        let (app, _, _) = app();
        let response = app.oneshot(get("/api/events")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await, serde_json::json!([]));
    }

    #[tokio::test]
    async fn post_events_appends_and_returns_the_stored_event() {
        let (app, room, _) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/api/events")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"kind":"match.result","actors":["tate"]}"#))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = json(response).await;
        assert_eq!(body["kind"], "match.result");
        assert!(body["id"].as_i64().unwrap() > 0);
        assert_eq!(room.events_since(0, -1).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn get_events_honours_after_and_limit() {
        let (app, room, _) = app();
        for i in 0..4 {
            room.record(crate::event::NewEvent::new(format!("k{i}"))).unwrap();
        }

        let response = app.oneshot(get("/api/events?after=1&limit=2")).await.unwrap();
        let body = json(response).await;
        let kinds: Vec<_> = body.as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["k1", "k2"]);
    }

    #[tokio::test]
    async fn get_roster_reflects_presence_events() {
        let (app, room, _) = app();
        room.record(crate::event::NewEvent::new(crate::event::kind::PRESENCE_ARRIVED).actor("sam"))
            .unwrap();

        let response = app.oneshot(get("/api/roster")).await.unwrap();
        assert_eq!(json(response).await, serde_json::json!(["sam"]));
    }

    #[tokio::test]
    async fn get_apps_lists_the_registry_and_nothing_current() {
        let (app, _, _) = app();
        let body = json(app.oneshot(get("/api/apps")).await.unwrap()).await;

        assert_eq!(body["apps"][0]["id"], "kodi");
        assert_eq!(body["apps"][0]["name"], "Kodi");
        assert_eq!(body["current"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn launching_an_app_returns_no_content_and_updates_current() {
        let (app, _, launcher) = app();
        let response = app.oneshot(post_empty("/api/apps/kodi/launch")).await.unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(launcher.current(), Some("kodi".to_string()));
    }

    #[tokio::test]
    async fn launching_an_unknown_app_is_not_found() {
        let (app, _, _) = app();
        let response = app.oneshot(post_empty("/api/apps/nintendo64/launch")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn quitting_returns_no_content_and_clears_current() {
        let (app, _, launcher) = app();
        launcher.launch("kodi").unwrap();

        let response = app.oneshot(post_empty("/api/apps/quit")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(launcher.current(), None);
    }
}
```

- [ ] **Step 2: Add the test-only dependencies**

Add to `crates/seshd/Cargo.toml` under `[dev-dependencies]`:

```toml
http-body-util = "0.1"
tower = { version = "0.4", features = ["util"] }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p seshd api`
Expected: FAIL — `cannot find type AppState`, `cannot find function router`.

- [ ] **Step 4: Write the handlers**

`crates/seshd/src/api/events.rs`:

```rust
//! Event log endpoints.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::event::{Event, NewEvent};

use super::AppState;

/// Query parameters for reading history.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Return events with an id greater than this. Defaults to 0.
    #[serde(default)]
    pub after: i64,
    /// Maximum number of events. Defaults to 500.
    pub limit: Option<i64>,
}

/// `GET /api/events` — read history.
pub async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<Vec<Event>>, StatusCode> {
    let limit = query.limit.unwrap_or(500);
    state
        .room
        .events_since(query.after, limit)
        .map(Json)
        .map_err(|error| {
            tracing::error!(%error, "reading events failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `POST /api/events` — the ingest port. Any producer may append here;
/// this is where the deferred game-capture strategy will plug in.
pub async fn post_event(
    State(state): State<AppState>,
    Json(new): Json<NewEvent>,
) -> Result<(StatusCode, Json<Event>), StatusCode> {
    state
        .room
        .record(new)
        .map(|event| (StatusCode::CREATED, Json(event)))
        .map_err(|error| {
            tracing::error!(%error, "recording event failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `GET /api/roster` — who is in the room.
pub async fn get_roster(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.room.roster())
}
```

`crates/seshd/src/api/apps.rs`:

```rust
//! App launcher endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::config::AppSpec;

use super::AppState;

/// The app registry plus what is running right now.
#[derive(Debug, Serialize)]
pub struct AppsResponse {
    /// Every launchable app, in registry order.
    pub apps: Vec<AppSpec>,
    /// The id of the running app, if any.
    pub current: Option<String>,
}

/// `GET /api/apps`
pub async fn list_apps(State(state): State<AppState>) -> Json<AppsResponse> {
    Json(AppsResponse {
        apps: state.launcher.apps().to_vec(),
        current: state.launcher.current(),
    })
}

/// `POST /api/apps/:id/launch`
pub async fn launch_app(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    if !state.launcher.apps().iter().any(|a| a.id == id) {
        return Err(StatusCode::NOT_FOUND);
    }
    state.launcher.launch(&id).map(|()| StatusCode::NO_CONTENT).map_err(|error| {
        tracing::error!(%error, %id, "launching app failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// `POST /api/apps/quit`
pub async fn quit_app(State(state): State<AppState>) -> Result<StatusCode, StatusCode> {
    state.launcher.quit().map(|()| StatusCode::NO_CONTENT).map_err(|error| {
        tracing::error!(%error, "quitting app failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}
```

- [ ] **Step 5: Write the router**

Insert into `crates/seshd/src/api/mod.rs`, between the `use` block and the test module:

```rust
/// Everything the API handlers need. Cheap to clone — both fields are `Arc`.
#[derive(Clone)]
pub struct AppState {
    /// The event log and its projections.
    pub room: Arc<Room>,
    /// The app launcher.
    pub launcher: Arc<Launcher>,
}

/// Build the API router. Static file serving is added by `main`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/events", get(events::list_events).post(events::post_event))
        .route("/api/roster", get(events::get_roster))
        .route("/api/apps", get(apps::list_apps))
        .route("/api/apps/:id/launch", post(apps::launch_app))
        .route("/api/apps/quit", post(apps::quit_app))
        .with_state(state)
}
```

Add to `crates/seshd/src/lib.rs`:

```rust
pub mod api;
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p seshd api`
Expected: PASS — 8 tests.

- [ ] **Step 7: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add crates/seshd/src/api crates/seshd/src/lib.rs crates/seshd/Cargo.toml
git commit -m "feat(api): add the HTTP API for events, roster, and apps

POST /api/events is the documented ingest port from the spec: the
deferred game-capture strategy plugs in here without touching the core."
```

---

### Task 9: The WebSocket live feed

**Files:**
- Create: `crates/seshd/src/api/ws.rs`
- Modify: `crates/seshd/src/api/mod.rs`
- Test: create `crates/seshd/tests/ws_feed.rs` (integration test — a real server on a real port)

**Interfaces:**
- Consumes: `AppState` (Task 8), `Room::subscribe` (Task 4).
- Produces: `ws::ws_handler` mounted at `GET /ws`. Each connected client receives every subsequent event as a JSON text frame.

**Why an integration test:** WebSocket upgrade cannot be exercised through `tower::ServiceExt::oneshot`. This test binds a real port and connects a real client, which works identically on Windows and the Pi.

- [ ] **Step 1: Write the failing test**

`crates/seshd/tests/ws_feed.rs`:

```rust
//! The live event feed, exercised against a real server on a real port.

use std::sync::Arc;

use futures_util::StreamExt;
use seshd::api::{router_with_ws, AppState};
use seshd::config::AppSpec;
use seshd::event::NewEvent;
use seshd::launcher::platform::MockPlatform;
use seshd::launcher::Launcher;
use seshd::room::Room;
use seshd::store::Store;

async fn serve() -> (String, Arc<Room>) {
    let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
    let launcher = Launcher::new(
        vec![AppSpec {
            id: "kodi".into(),
            name: "Kodi".into(),
            command: "kodi".into(),
            args: vec![],
            icon: "movie".into(),
        }],
        Arc::new(MockPlatform::new()),
        room.clone(),
    );
    let app = router_with_ws(AppState {
        room: room.clone(),
        launcher,
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("ws://{addr}/ws"), room)
}

#[tokio::test]
async fn a_connected_client_receives_events_recorded_after_it_connects() {
    let (url, room) = serve().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    room.record(NewEvent::new("app.launched").subject("kodi")).unwrap();

    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("timed out waiting for an event")
        .expect("socket closed")
        .unwrap();

    let event: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert_eq!(event["kind"], "app.launched");
    assert_eq!(event["subject"], "kodi");
}

#[tokio::test]
async fn two_clients_both_receive_the_same_event() {
    let (url, room) = serve().await;
    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    room.record(NewEvent::new("moment.captured").subject("clip-1")).unwrap();

    for socket in [&mut a, &mut b] {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("timed out")
            .expect("socket closed")
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(event["subject"], "clip-1");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p seshd --test ws_feed`
Expected: FAIL — `cannot find function router_with_ws`.

- [ ] **Step 3: Write the handler**

`crates/seshd/src/api/ws.rs`:

```rust
//! The live event feed.
//!
//! Every surface — the TV and, from Arc 3, phones — holds one of these
//! sockets open and re-renders from it. Clients that fall behind the
//! broadcast backlog are dropped and expected to reconnect and catch up
//! via `GET /api/events?after=<last_id>`.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;

use super::AppState;

/// `GET /ws` — upgrade to a live event feed.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let events = state.room.subscribe();
    ws.on_upgrade(move |socket| pump(socket, events))
}

async fn pump(mut socket: WebSocket, mut events: tokio::sync::broadcast::Receiver<crate::event::Event>) {
    loop {
        match events.recv().await {
            Ok(event) => {
                let Ok(text) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(text)).await.is_err() {
                    return; // client hung up
                }
            }
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "surface fell behind the event feed");
            }
            Err(RecvError::Closed) => return,
        }
    }
}
```

- [ ] **Step 4: Mount it**

In `crates/seshd/src/api/mod.rs`, add `pub mod ws;` beside the other module declarations, and add this function below `router`:

```rust
/// The API router plus the live event feed. `router` alone is kept for
/// tests that use `oneshot`, which cannot perform a WebSocket upgrade.
pub fn router_with_ws(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws::ws_handler))
        .with_state(state.clone())
        .merge(router(state))
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p seshd --test ws_feed`
Expected: PASS — 2 tests.

- [ ] **Step 6: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add crates/seshd/src/api/ws.rs crates/seshd/src/api/mod.rs crates/seshd/tests/ws_feed.rs
git commit -m "feat(api): add the live event feed over websocket

Surfaces render from this rather than polling, which is what lets the TV
and phones stay in sync in Arc 3 with no extra machinery."
```

---

### Task 10: The binary — CLI, wiring, and static serving

**Files:**
- Modify: `crates/seshd/src/main.rs`
- Test: manual — this task is wiring, verified by running the binary. No unit test.

**Interfaces:**
- Consumes: everything above.
- Produces: a `seshd` binary accepting `--db <path>`, `--apps <path>`, `--static <dir>`, `--bind <addr>`.

- [ ] **Step 1: Write the binary**

Replace `crates/seshd/src/main.rs`:

```rust
//! Binary entry point: parse arguments, wire the daemon, serve.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use seshd::api::{router_with_ws, AppState};
use seshd::config::load_apps_file;
use seshd::launcher::platform::ProcessPlatform;
use seshd::launcher::Launcher;
use seshd::room::Room;
use seshd::store::Store;
use tower_http::services::{ServeDir, ServeFile};

/// The SESH room daemon.
#[derive(Debug, Parser)]
#[command(name = "seshd", version)]
struct Args {
    /// Path to the event log database. Created if absent.
    #[arg(long, default_value = "sesh.db")]
    db: PathBuf,

    /// Path to the app registry.
    #[arg(long, default_value = "deploy/apps.toml")]
    apps: PathBuf,

    /// Directory holding the built surface bundle.
    #[arg(long, default_value = "surfaces/dist")]
    r#static: PathBuf,

    /// Address to bind.
    #[arg(long, default_value = "0.0.0.0:7373")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "seshd=info,tower_http=warn".into()),
        )
        .init();

    let args = Args::parse();

    let store = Store::open(&args.db).with_context(|| format!("opening {}", args.db.display()))?;
    let room = Room::new(store)?;
    let apps = load_apps_file(&args.apps)?;
    tracing::info!(count = apps.len(), "loaded app registry");

    let launcher = Launcher::new(apps, Arc::new(ProcessPlatform::new()), room.clone());
    tokio::spawn(Launcher::reap_loop(launcher.clone()));

    // Anything not under /api or /ws falls through to the surface bundle,
    // and unknown paths serve index.html so the surface owns its routing.
    let index = args.r#static.join("index.html");
    let surface = ServeDir::new(&args.r#static).not_found_service(ServeFile::new(index));

    let app = router_with_ws(AppState { room, launcher }).fallback_service(surface);

    let listener = tokio::net::TcpListener::bind(&args.bind)
        .await
        .with_context(|| format!("binding {}", args.bind))?;
    tracing::info!(addr = %listener.local_addr()?, "seshd listening");

    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build -p seshd`
Expected: success.

- [ ] **Step 3: Verify it runs and serves the API**

In one terminal:

```bash
cargo run -p seshd -- --db ./scratch.db --apps deploy/apps.toml --bind 127.0.0.1:7373
```

In another:

```bash
curl -s http://127.0.0.1:7373/api/apps
curl -s -X POST http://127.0.0.1:7373/api/events -H "content-type: application/json" -d '{"kind":"presence.arrived","actors":["tate"]}'
curl -s http://127.0.0.1:7373/api/roster
```

Expected: the app list with `"current": null`; then a `201` echoing the stored event with an `id`; then `["tate"]`.

Stop the server and delete the scratch database:

```bash
rm -f scratch.db scratch.db-wal scratch.db-shm
```

- [ ] **Step 4: Run the gate**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/seshd/src/main.rs
git commit -m "feat(core): wire the seshd binary and serve the surface bundle

Unknown paths fall through to index.html so the surface owns its own
routing without seshd needing to know any of it."
```

---

### Task 11: The surface — scaffold, API client, and navigation

**Files:**
- Create: `surfaces/package.json`, `surfaces/tsconfig.json`, `surfaces/vite.config.ts`, `surfaces/index.html`, `surfaces/src/api.ts`, `surfaces/src/nav.ts`
- Test: create `surfaces/src/api.test.ts`, `surfaces/src/nav.test.ts`

**Interfaces:**
- Consumes: the HTTP/WS API from Tasks 8 and 9.
- Produces: `listApps(fetchFn?): Promise<AppsResponse>`, `launchApp(id, fetchFn?): Promise<void>`, `quitApp(fetchFn?): Promise<void>`, `connectEvents(onEvent, WsCtor?): () => void`; `move(index, count, columns, dir): number`.

**Why `fetchFn` and `WsCtor` are injectable:** it keeps every client test a pure unit test with no server and no DOM globals to stub.

- [ ] **Step 1: Scaffold the project**

`surfaces/package.json`:

```json
{
  "name": "sesh-surfaces",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "test": "vitest run"
  },
  "devDependencies": {
    "typescript": "^5.4.0",
    "vite": "^5.2.0",
    "vitest": "^1.6.0"
  }
}
```

`surfaces/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "isolatedModules": true,
    "skipLibCheck": true,
    "types": ["vite/client"]
  },
  "include": ["src"]
}
```

`surfaces/vite.config.ts`:

```ts
import { defineConfig } from "vite";

export default defineConfig({
  server: {
    proxy: {
      "/api": "http://127.0.0.1:7373",
      "/ws": { target: "ws://127.0.0.1:7373", ws: true },
    },
  },
  build: { outDir: "dist", emptyOutDir: true },
});
```

`surfaces/index.html`:

```html
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>SESH</title>
<div id="app"></div>
<script type="module" src="/src/main.ts"></script>
```

Run: `cd surfaces && npm install`

- [ ] **Step 2: Write the failing tests**

`surfaces/src/nav.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { move } from "./nav";

describe("move", () => {
  // A 5-item grid, 3 columns:  0 1 2
  //                            3 4
  const COUNT = 5;
  const COLUMNS = 3;

  it("moves right within a row", () => {
    expect(move(0, COUNT, COLUMNS, "right")).toBe(1);
  });

  it("stops at the last item instead of wrapping", () => {
    expect(move(4, COUNT, COLUMNS, "right")).toBe(4);
  });

  it("stops at the first item instead of wrapping", () => {
    expect(move(0, COUNT, COLUMNS, "left")).toBe(0);
  });

  it("moves down a full row", () => {
    expect(move(0, COUNT, COLUMNS, "down")).toBe(3);
  });

  it("clamps to the last item when the row below is short", () => {
    expect(move(2, COUNT, COLUMNS, "down")).toBe(4);
  });

  it("moves up a full row", () => {
    expect(move(3, COUNT, COLUMNS, "up")).toBe(0);
  });

  it("stays put when there is no row above", () => {
    expect(move(1, COUNT, COLUMNS, "up")).toBe(1);
  });

  it("returns 0 for an empty grid", () => {
    expect(move(0, 0, COLUMNS, "right")).toBe(0);
  });
});
```

`surfaces/src/api.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { connectEvents, launchApp, listApps, quitApp } from "./api";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

describe("listApps", () => {
  it("requests the apps endpoint and returns the parsed body", async () => {
    const fetchFn = vi.fn().mockResolvedValue(
      jsonResponse({ apps: [{ id: "kodi", name: "Kodi", command: "kodi", args: [], icon: "movie" }], current: null }),
    );

    const result = await listApps(fetchFn as unknown as typeof fetch);

    expect(fetchFn).toHaveBeenCalledWith("/api/apps");
    expect(result.apps[0].id).toBe("kodi");
    expect(result.current).toBeNull();
  });

  it("throws when the server errors", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response("nope", { status: 500 }));
    await expect(listApps(fetchFn as unknown as typeof fetch)).rejects.toThrow(/500/);
  });
});

describe("launchApp", () => {
  it("posts to the launch endpoint", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    await launchApp("kodi", fetchFn as unknown as typeof fetch);
    expect(fetchFn).toHaveBeenCalledWith("/api/apps/kodi/launch", { method: "POST" });
  });

  it("throws when the app is unknown", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response(null, { status: 404 }));
    await expect(launchApp("n64", fetchFn as unknown as typeof fetch)).rejects.toThrow(/404/);
  });
});

describe("quitApp", () => {
  it("posts to the quit endpoint", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    await quitApp(fetchFn as unknown as typeof fetch);
    expect(fetchFn).toHaveBeenCalledWith("/api/apps/quit", { method: "POST" });
  });
});

describe("connectEvents", () => {
  it("parses incoming frames and hands them to the callback", () => {
    let onmessage: ((e: { data: string }) => void) | null = null;
    const close = vi.fn();
    const FakeWs = vi.fn(function (this: Record<string, unknown>) {
      this.close = close;
      Object.defineProperty(this, "onmessage", {
        set: (fn) => { onmessage = fn; },
      });
    });

    const received: unknown[] = [];
    const disconnect = connectEvents((e) => received.push(e), FakeWs as unknown as typeof WebSocket);

    onmessage!({ data: JSON.stringify({ id: 1, kind: "app.launched", subject: "kodi" }) });

    expect(received).toHaveLength(1);
    expect((received[0] as { kind: string }).kind).toBe("app.launched");

    disconnect();
    expect(close).toHaveBeenCalled();
  });
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd surfaces && npm test`
Expected: FAIL — cannot resolve `./nav` or `./api`.

- [ ] **Step 4: Write the navigation logic**

`surfaces/src/nav.ts`:

```ts
/** Grid navigation. Pure logic, deliberately separate from the DOM. */

export type Dir = "up" | "down" | "left" | "right";

/**
 * The index selected after moving `dir` from `index` in a `columns`-wide
 * grid of `count` items. Movement clamps rather than wraps: on a TV,
 * wrapping loses people.
 */
export function move(index: number, count: number, columns: number, dir: Dir): number {
  if (count <= 0) return 0;

  const clamp = (i: number) => Math.max(0, Math.min(count - 1, i));

  switch (dir) {
    case "left":
      return index % columns === 0 ? index : clamp(index - 1);
    case "right":
      return (index + 1) % columns === 0 || index + 1 >= count ? index : clamp(index + 1);
    case "up":
      return index - columns < 0 ? index : index - columns;
    case "down":
      return index + columns >= count
        ? (Math.floor(index / columns) + 1) * columns >= count
          ? index
          : clamp(count - 1)
        : index + columns;
  }
}
```

- [ ] **Step 5: Write the API client**

`surfaces/src/api.ts`:

```ts
/** Typed client for seshd. Mirrors `crates/seshd/src/api`. */

/** One launchable app. Mirrors `AppSpec` in `config.rs`. */
export interface AppSpec {
  id: string;
  name: string;
  command: string;
  args: string[];
  icon: string;
}

/** Response of `GET /api/apps`. */
export interface AppsResponse {
  apps: AppSpec[];
  current: string | null;
}

/** One recorded event. Mirrors `Event` in `event.rs`. */
export interface SeshEvent {
  id: number;
  ts_ms: number;
  kind: string;
  actors: string[];
  subject: string | null;
  payload: unknown;
}

async function ok(response: Response): Promise<Response> {
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`);
  }
  return response;
}

/** Fetch the app registry and what is running. */
export async function listApps(fetchFn: typeof fetch = fetch): Promise<AppsResponse> {
  const response = await ok(await fetchFn("/api/apps"));
  return (await response.json()) as AppsResponse;
}

/** Launch an app by id. */
export async function launchApp(id: string, fetchFn: typeof fetch = fetch): Promise<void> {
  await ok(await fetchFn(`/api/apps/${id}/launch`, { method: "POST" }));
}

/** Quit whatever is running. */
export async function quitApp(fetchFn: typeof fetch = fetch): Promise<void> {
  await ok(await fetchFn("/api/apps/quit", { method: "POST" }));
}

/**
 * Subscribe to the live event feed. Returns a function that disconnects.
 * The socket URL is derived from the page so this works identically
 * against the Vite dev proxy and against seshd on the Pi.
 */
export function connectEvents(
  onEvent: (event: SeshEvent) => void,
  WsCtor: typeof WebSocket = WebSocket,
): () => void {
  const protocol = typeof location !== "undefined" && location.protocol === "https:" ? "wss" : "ws";
  const host = typeof location !== "undefined" ? location.host : "localhost:7373";
  const socket = new WsCtor(`${protocol}://${host}/ws`);

  socket.onmessage = (message: MessageEvent) => {
    try {
      onEvent(JSON.parse(message.data as string) as SeshEvent);
    } catch {
      // A frame we cannot parse is not worth tearing the feed down for.
    }
  };

  return () => socket.close();
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd surfaces && npm test`
Expected: PASS — 8 nav tests, 6 api tests.

- [ ] **Step 7: Commit**

```bash
git add surfaces/
git commit -m "feat(surfaces): add the surface scaffold, API client, and grid nav

Navigation clamps rather than wraps: wrapping is disorienting at ten feet
with a controller. fetch and WebSocket are injectable so every client
test stays a pure unit test."
```

---

### Task 12: The home screen

**Files:**
- Create: `surfaces/src/views/home.ts`, `surfaces/src/styles.css`
- Modify: `surfaces/src/main.ts` (create), `surfaces/index.html`
- Test: create `surfaces/src/views/home.test.ts`

**Interfaces:**
- Consumes: `listApps`, `launchApp`, `quitApp`, `connectEvents`, `AppSpec`, `SeshEvent` (Task 11); `move`, `Dir` (Task 11).
- Produces: `renderHome(root: HTMLElement, state: HomeState): void`; `interface HomeState { apps: AppSpec[]; current: string | null; selected: number }`.

**Design note:** the TV surface is rendered with plain DOM string assembly. At Arc 1's size a framework earns nothing, and a Pi's browser is happier with less. Arc 2 revisits this when attract mode needs animation.

- [ ] **Step 1: Write the failing test**

`surfaces/src/views/home.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { renderHome } from "./home";
import type { AppSpec } from "../api";

const APPS: AppSpec[] = [
  { id: "kodi", name: "Kodi", command: "kodi", args: [], icon: "movie" },
  { id: "retroarch", name: "RetroArch", command: "retroarch", args: [], icon: "gamepad" },
];

function root(): HTMLElement {
  return document.createElement("div");
}

describe("renderHome", () => {
  it("renders a tile per app", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 0 });

    const tiles = el.querySelectorAll("[data-app-id]");
    expect(tiles).toHaveLength(2);
    expect(tiles[0].getAttribute("data-app-id")).toBe("kodi");
    expect(tiles[0].textContent).toContain("Kodi");
  });

  it("marks exactly one tile selected", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 1 });

    const selected = el.querySelectorAll(".tile--selected");
    expect(selected).toHaveLength(1);
    expect(selected[0].getAttribute("data-app-id")).toBe("retroarch");
  });

  it("marks the running app", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: "kodi", selected: 0 });

    const running = el.querySelector(".tile--running");
    expect(running?.getAttribute("data-app-id")).toBe("kodi");
  });

  it("shows a quit hint only while something is running", () => {
    const idle = root();
    renderHome(idle, { apps: APPS, current: null, selected: 0 });
    expect(idle.textContent).not.toContain("Quit");

    const busy = root();
    renderHome(busy, { apps: APPS, current: "kodi", selected: 0 });
    expect(busy.textContent).toContain("Quit");
  });

  it("shows a message when the registry is empty", () => {
    const el = root();
    renderHome(el, { apps: [], current: null, selected: 0 });
    expect(el.textContent).toContain("No apps configured");
  });

  it("escapes app names so a registry cannot inject markup", () => {
    const el = root();
    renderHome(el, {
      apps: [{ id: "x", name: "<img src=x onerror=alert(1)>", command: "x", args: [], icon: "" }],
      current: null,
      selected: 0,
    });
    expect(el.querySelector("img")).toBeNull();
  });
});
```

Add `jsdom` so DOM tests can run:

```bash
cd surfaces && npm install -D jsdom
```

Add to `surfaces/vite.config.ts` inside `defineConfig({...})`:

```ts
  test: { environment: "jsdom" },
```

and change its import line to:

```ts
/// <reference types="vitest" />
import { defineConfig } from "vite";
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd surfaces && npm test`
Expected: FAIL — cannot resolve `./home`.

- [ ] **Step 3: Write the view**

`surfaces/src/views/home.ts`:

```ts
/** The front door: a grid of launchable apps, navigable with a controller. */

import type { AppSpec } from "../api";

/** Everything the home screen renders from. */
export interface HomeState {
  apps: AppSpec[];
  current: string | null;
  selected: number;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Render the home screen into `root`, replacing its contents. */
export function renderHome(root: HTMLElement, state: HomeState): void {
  if (state.apps.length === 0) {
    root.innerHTML = `<main class="home"><p class="empty">No apps configured. Check apps.toml.</p></main>`;
    return;
  }

  const tiles = state.apps
    .map((app, index) => {
      const classes = [
        "tile",
        index === state.selected ? "tile--selected" : "",
        app.id === state.current ? "tile--running" : "",
      ]
        .filter(Boolean)
        .join(" ");

      return `<button class="${classes}" data-app-id="${escapeHtml(app.id)}">
        <span class="tile__icon" data-icon="${escapeHtml(app.icon)}"></span>
        <span class="tile__name">${escapeHtml(app.name)}</span>
      </button>`;
    })
    .join("");

  const hint = state.current
    ? `<p class="hint">${escapeHtml(state.current)} is running — press B or Backspace to Quit</p>`
    : `<p class="hint">Select an app</p>`;

  root.innerHTML = `<main class="home"><h1 class="wordmark">SESH</h1><div class="grid">${tiles}</div>${hint}</main>`;
}

/** Tile columns. Kept here so `main.ts` and the CSS agree on one number. */
export const COLUMNS = 3;
```

- [ ] **Step 4: Write the stylesheet**

`surfaces/src/styles.css`:

```css
/* Ten-foot UI: large type, high contrast, obvious focus. */

:root {
  --bg: #0b0b0f;
  --fg: #f2f2f7;
  --dim: #8a8a99;
  --accent: #7c5cff;
  --running: #2fbf71;
}

* { box-sizing: border-box; }

body {
  margin: 0;
  background: var(--bg);
  color: var(--fg);
  font: 400 1rem/1.4 system-ui, sans-serif;
  overflow: hidden;
}

.home {
  height: 100vh;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 3vh;
  padding: 4vw;
}

.wordmark {
  margin: 0;
  font-size: 3vw;
  letter-spacing: 0.4em;
  color: var(--dim);
}

.grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 2vw;
}

.tile {
  aspect-ratio: 16 / 10;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1vh;
  border: 0.4vh solid transparent;
  border-radius: 1.5vh;
  background: #16161f;
  color: inherit;
  font-size: 2vw;
  cursor: pointer;
  transition: transform 120ms ease, border-color 120ms ease;
}

.tile--selected {
  border-color: var(--accent);
  transform: scale(1.05);
}

.tile--running { box-shadow: inset 0 -0.8vh 0 var(--running); }

.tile__icon::before {
  content: "\25B6";
  font-size: 3vw;
  color: var(--dim);
}
.tile__icon[data-icon="movie"]::before { content: "\1F3AC"; }
.tile__icon[data-icon="gamepad"]::before { content: "\1F579"; }
.tile__icon[data-icon="display"]::before { content: "\1F5A5"; }

.hint, .empty { color: var(--dim); font-size: 1.4vw; margin: 0; }
```

- [ ] **Step 5: Write the bootstrap and input loop**

`surfaces/src/main.ts`:

```ts
/** Bootstrap: load state, render, and drive selection from a controller. */

import "./styles.css";
import { connectEvents, launchApp, listApps, quitApp, type SeshEvent } from "./api";
import { move, type Dir } from "./nav";
import { COLUMNS, renderHome, type HomeState } from "./views/home";

const root = document.getElementById("app")!;

const state: HomeState = { apps: [], current: null, selected: 0 };

function draw(): void {
  renderHome(root, state);
}

async function refresh(): Promise<void> {
  const { apps, current } = await listApps();
  state.apps = apps;
  state.current = current;
  state.selected = Math.min(state.selected, Math.max(0, apps.length - 1));
  draw();
}

function navigate(dir: Dir): void {
  state.selected = move(state.selected, state.apps.length, COLUMNS, dir);
  draw();
}

async function activate(): Promise<void> {
  const app = state.apps[state.selected];
  if (app) await launchApp(app.id);
}

const KEYS: Record<string, () => void | Promise<void>> = {
  ArrowUp: () => navigate("up"),
  ArrowDown: () => navigate("down"),
  ArrowLeft: () => navigate("left"),
  ArrowRight: () => navigate("right"),
  Enter: activate,
  Backspace: quitApp,
};

window.addEventListener("keydown", (e) => {
  const handler = KEYS[e.key];
  if (handler) {
    e.preventDefault();
    void handler();
  }
});

root.addEventListener("click", (e) => {
  const tile = (e.target as HTMLElement).closest("[data-app-id]");
  if (tile) void launchApp(tile.getAttribute("data-app-id")!);
});

// Gamepad: the Gamepad API has no event for button presses, so it must be
// polled. Edge-detect against the previous frame so a held button fires once.
const GAMEPAD_ACTIONS: Array<[number, () => void | Promise<void>]> = [
  [12, () => navigate("up")],
  [13, () => navigate("down")],
  [14, () => navigate("left")],
  [15, () => navigate("right")],
  [0, activate],
  [1, quitApp],
];

let previous: boolean[] = [];

function pollGamepad(): void {
  const pad = navigator.getGamepads?.().find((p) => p !== null);
  if (pad) {
    for (const [button, action] of GAMEPAD_ACTIONS) {
      const pressed = pad.buttons[button]?.pressed ?? false;
      if (pressed && !previous[button]) void action();
      previous[button] = pressed;
    }
  }
  requestAnimationFrame(pollGamepad);
}

connectEvents((event: SeshEvent) => {
  if (event.kind === "app.launched" || event.kind === "app.exited") {
    void refresh();
  }
});

void refresh();
requestAnimationFrame(pollGamepad);
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd surfaces && npm test`
Expected: PASS — 8 nav, 6 api, 6 home.

- [ ] **Step 7: Verify the build and see it in a browser**

```bash
cd surfaces && npm run build
```
Expected: `dist/` produced with no TypeScript errors.

Then, with `seshd` running from Task 10 (`cargo run -p seshd -- --db ./scratch.db --apps deploy/apps.toml --bind 127.0.0.1:7373`), run `npm run dev` and open the printed URL. Confirm: three tiles render; arrow keys move the purple selection border and it clamps at the edges rather than wrapping. Pressing Enter attempts a launch and fails — Kodi is not installed on the dev machine — which is expected here and is verified for real in Task 13.

- [ ] **Step 8: Commit**

```bash
git add surfaces/
git commit -m "feat(surfaces): add the home screen and controller input

Plain DOM rather than a framework: Arc 1's surface is a grid, and a Pi's
browser is happier with less. Gamepad buttons are edge-detected against
the previous frame so a held d-pad does not scroll away."
```

---

### Task 13: Pi deployment and end-to-end verification

**Files:**
- Create: `deploy/seshd.service`, `deploy/labwc/autostart`, `deploy/labwc/rc.xml`, `deploy/install.sh`, `deploy/README.md`
- Test: manual, on the Pi. Per the spec, deployment and compositor behavior are hardware-bound and verified by running, not asserting.

**Interfaces:**
- Consumes: the `seshd` binary (Task 10), the built surface bundle (Task 12), `deploy/apps.toml` (Task 6).
- Produces: a Pi that boots into SESH.

- [ ] **Step 1: Write the systemd user unit**

`deploy/seshd.service`:

```ini
[Unit]
Description=SESH room daemon
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/usr/local/bin/seshd \
  --db %h/.local/share/sesh/sesh.db \
  --apps /etc/sesh/apps.toml \
  --static /usr/local/share/sesh/web \
  --bind 0.0.0.0:7373
Restart=always
RestartSec=2
Environment=RUST_LOG=seshd=info

[Install]
WantedBy=graphical-session.target
```

- [ ] **Step 2: Write the compositor configuration**

`deploy/labwc/autostart`:

```sh
#!/bin/sh
# labwc runs this once the compositor is up.

# Hand the session's Wayland environment to the user systemd manager, so
# seshd and every app it spawns can reach the display.
systemctl --user import-environment WAYLAND_DISPLAY XDG_RUNTIME_DIR
systemctl --user start seshd.service

# Wait for seshd to accept connections before pointing the browser at it.
for _ in $(seq 1 50); do
    if curl -sf http://127.0.0.1:7373/api/apps >/dev/null 2>&1; then break; fi
    sleep 0.2
done

chromium-browser \
    --kiosk \
    --ozone-platform=wayland \
    --enable-features=UseOzonePlatform \
    --noerrdialogs \
    --disable-infobars \
    --disable-session-crashed-bubble \
    --check-for-update-interval=31536000 \
    --autoplay-policy=no-user-gesture-required \
    http://127.0.0.1:7373 &
```

`deploy/labwc/rc.xml`:

```xml
<?xml version="1.0"?>
<!-- Undecorated, fullscreen, no user-visible window management. Launched
     apps stack over the SESH kiosk; killing one reveals SESH again. -->
<labwc_config>
  <core>
    <gap>0</gap>
  </core>
  <theme>
    <titlebar>
      <layout></layout>
    </titlebar>
  </theme>
  <windowRules>
    <windowRule identifier="*">
      <serverDecoration>no</serverDecoration>
    </windowRule>
  </windowRules>
</labwc_config>
```

- [ ] **Step 3: Write the install script**

`deploy/install.sh`:

```sh
#!/bin/sh
# Provision a Raspberry Pi 5 running Raspberry Pi OS Lite (64-bit) to boot
# into SESH. Run as root on the Pi, from the repo root:
#     sudo sh deploy/install.sh
set -eu

SESH_USER="${SESH_USER:-sesh}"

echo "==> Installing packages"
apt-get update
apt-get install -y labwc chromium-browser seatd curl kodi retroarch

echo "==> Creating user ${SESH_USER}"
id -u "$SESH_USER" >/dev/null 2>&1 || useradd -m -G video,input,render,audio "$SESH_USER"
loginctl enable-linger "$SESH_USER"

echo "==> Installing binary, surface bundle, and configuration"
install -Dm755 target/aarch64-unknown-linux-gnu/release/seshd /usr/local/bin/seshd
install -Dm644 deploy/apps.toml /etc/sesh/apps.toml
rm -rf /usr/local/share/sesh/web
mkdir -p /usr/local/share/sesh/web
cp -r surfaces/dist/. /usr/local/share/sesh/web/

SESH_HOME="$(getent passwd "$SESH_USER" | cut -d: -f6)"
install -Dm644 deploy/seshd.service "${SESH_HOME}/.config/systemd/user/seshd.service"
install -Dm644 deploy/labwc/rc.xml "${SESH_HOME}/.config/labwc/rc.xml"
install -Dm755 deploy/labwc/autostart "${SESH_HOME}/.config/labwc/autostart"
mkdir -p "${SESH_HOME}/.local/share/sesh"

echo "==> Starting labwc on login to tty1"
cat > "${SESH_HOME}/.bash_profile" <<'EOF'
if [ -z "${WAYLAND_DISPLAY:-}" ] && [ "$(tty)" = "/dev/tty1" ]; then
    exec labwc
fi
EOF
chown -R "$SESH_USER:$SESH_USER" "$SESH_HOME"

echo "==> Enabling autologin on tty1"
mkdir -p /etc/systemd/system/getty@tty1.service.d
cat > /etc/systemd/system/getty@tty1.service.d/autologin.conf <<EOF
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin ${SESH_USER} --noclear %I \$TERM
EOF
systemctl daemon-reload

echo "==> Done. Edit /etc/sesh/apps.toml (set the Moonlight host), then reboot."
```

- [ ] **Step 4: Write the deployment notes**

`deploy/README.md`:

````markdown
# Deploying SESH to the Pi

## Build on the dev machine

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu -p seshd
cd surfaces && npm ci && npm run build && cd ..
```

Cross-linking needs an aarch64 linker. On Windows the simplest route is to
build on the Pi itself (`cargo build --release` there, then adjust the
`install -Dm755` source path in `install.sh` to `target/release/seshd`).

## Install on the Pi

Copy the repo to the Pi, then:

```bash
sudo sh deploy/install.sh
sudo nano /etc/sesh/apps.toml   # set the Moonlight host
sudo reboot
```

## What should happen

The Pi boots to tty1, autologs in as `sesh`, starts labwc, which starts
`seshd` and then Chromium in kiosk mode pointed at `http://127.0.0.1:7373`.

## Checking on it

```bash
systemctl --user status seshd      # as the sesh user
journalctl --user -u seshd -f
curl -s http://<pi-address>:7373/api/apps
```

The API is reachable from anywhere on the LAN, which is how phones will
attach in Arc 3.
````

- [ ] **Step 5: Install and reboot the Pi**

Follow `deploy/README.md`. Then `sudo reboot`.

- [ ] **Step 6: Verify the boot path**

Confirm each, in order:

1. The Pi boots to the SESH home screen with no desktop, no cursor, and no browser chrome visible.
2. `systemctl --user status seshd` (as `sesh`) reports `active (running)`.
3. Three tiles are visible: Kodi, RetroArch, Moonlight.
4. A controller's d-pad moves the selection border; it clamps at the grid edges rather than wrapping.

- [ ] **Step 7: Verify launch and return for each app**

For each of Kodi, RetroArch, and Moonlight, confirm all four:

1. Selecting the tile and pressing A starts the app fullscreen over SESH.
2. `curl -s http://127.0.0.1:7373/api/apps` reports that app as `current`.
3. Quitting the app **from inside the app's own menu** returns to the SESH home screen within about a second, and `current` becomes `null`. *(This is the reaper from Task 7. If `current` stays stuck, the reaper is not running.)*
4. Relaunching the app, then pressing B on the controller, also returns to SESH and clears `current`.

- [ ] **Step 8: Verify the log recorded the evening**

```bash
curl -s http://127.0.0.1:7373/api/events | python3 -m json.tool
```

Expected: alternating `app.launched` and `app.exited` events, in the order performed, each with the app id as `subject` and a plausible `ts_ms`. This is Arc 1's real deliverable — the room now has a memory.

- [ ] **Step 9: Verify the log survives a reboot**

```bash
sudo reboot
# after it comes back:
curl -s http://127.0.0.1:7373/api/events | python3 -m json.tool
```

Expected: every event from before the reboot is still there.

- [ ] **Step 10: Commit**

```bash
git add deploy/
git commit -m "feat(deploy): boot the Pi into SESH

seshd runs as a systemd user service started from labwc's autostart, not
as a system service, so apps it spawns inherit WAYLAND_DISPLAY from the
compositor session and actually appear on the TV."
```

---

## Definition of Done

Arc 1 is complete when all of the following hold:

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` are green.
- `cd surfaces && npm test && npm run build` is green.
- The Pi boots straight to the SESH home screen.
- Kodi, RetroArch, and Moonlight each launch and return — whether quit from the controller or from inside the app itself.
- `GET /api/events` shows the whole session, and it survives a reboot.

## What Arc 1 deliberately does not do

Presence detection, phones, the trophy case, attract mode content, audio routing, and clip capture are all later Arcs. Arc 1 ships the log, the shell, and the launcher — and `POST /api/events` already accepts anything those Arcs will want to record.
