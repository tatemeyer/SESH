//! The identity registry.
//!
//! `people` is the one table in SESH that is not a projection. It is source
//! data — who exists, and the token their phone authenticates with — so it may
//! be updated and migrated, and it is deliberately exempt from the
//! "rebuildable from the log alone" invariant. The append-only rule covers
//! `events` only.
//!
//! Tokens live in this table and **never** in the log. The log is append-only
//! and served unauthenticated on the LAN; nothing that must ever stop being
//! true belongs in it.

use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::Serialize;

use super::{now_ms, Store};

/// Someone the house knows.
///
/// Deliberately has no `token` field. This type is handed to HTTP handlers and
/// serialized to clients, so the token is not merely omitted from responses —
/// it is not reachable from here at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Person {
    /// Stable id, a slug of the display name. Appears in event `actors`.
    pub id: String,
    /// What they asked to be called.
    pub name: String,
    /// Optional avatar reference. Unused so far.
    pub avatar: Option<String>,
    /// When they first joined, in Unix milliseconds.
    pub joined_ms: i64,
}

/// How many `name-2`, `name-3`, ... variants to try before giving up.
const MAX_ID_ATTEMPTS: u32 = 200;

/// Longest slug before the suffix. A display name is free-form; a primary key
/// that inherits its length is not something to find out about later.
const MAX_SLUG_LEN: usize = 32;

/// Columns selected by every read here, in the order [`row_to_person`] expects.
const COLUMNS: &str = "id, name, avatar, joined_ms";

fn row_to_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<Person> {
    Ok(Person {
        id: row.get(0)?,
        name: row.get(1)?,
        avatar: row.get(2)?,
        joined_ms: row.get(3)?,
    })
}

impl Store {
    /// Register a person and return them, allocating an id from their name.
    ///
    /// The id is a slug of `name`, suffixed if that slug is taken.
    pub fn insert_person(&self, name: &str, token: &str) -> Result<Person> {
        let base = slugify(name);
        let joined_ms = now_ms();
        let conn = self.conn.lock().expect("store mutex poisoned");

        for attempt in 1..=MAX_ID_ATTEMPTS {
            let id = if attempt == 1 {
                base.clone()
            } else {
                format!("{base}-{attempt}")
            };

            let result = conn.execute(
                "INSERT INTO people (id, name, avatar, token, joined_ms)
                 VALUES (?1, ?2, NULL, ?3, ?4)",
                rusqlite::params![id, name, token, joined_ms],
            );

            match result {
                Ok(_) => {
                    return Ok(Person {
                        id,
                        name: name.to_string(),
                        avatar: None,
                        joined_ms,
                    })
                }
                // Only an id collision is worth retrying. Matching the column
                // name matters: a token collision would otherwise burn all 200
                // attempts and then report the wrong cause. Tokens are 128 bits
                // of randomness, so a duplicate means a caller bug, and a
                // caller bug should surface as itself.
                Err(rusqlite::Error::SqliteFailure(error, Some(ref message)))
                    if error.code == rusqlite::ErrorCode::ConstraintViolation
                        && message.contains("people.id") => {}
                Err(error) => return Err(error.into()),
            }
        }

        Err(anyhow!(
            "could not allocate an id for {name:?}: {base} and \
             {MAX_ID_ATTEMPTS} suffixed variants are all taken"
        ))
    }

    /// Resolve a phone token to whoever holds it.
    pub fn person_by_token(&self, token: &str) -> Result<Option<Person>> {
        self.person_where("token = ?1", token)
    }

    /// Look someone up by id.
    pub fn person_by_id(&self, id: &str) -> Result<Option<Person>> {
        self.person_where("id = ?1", id)
    }

    fn person_where(&self, predicate: &str, value: &str) -> Result<Option<Person>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let sql = format!("SELECT {COLUMNS} FROM people WHERE {predicate}");
        let found = conn
            .query_row(&sql, rusqlite::params![value], row_to_person)
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(found)
    }

    /// Everyone the house knows, oldest join first.
    pub fn people(&self) -> Result<Vec<Person>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        // Two people can join inside the same millisecond, so `joined_ms`
        // alone is not a total order. `rowid` breaks the tie by insertion.
        let sql = format!("SELECT {COLUMNS} FROM people ORDER BY joined_ms ASC, rowid ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_person)?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

/// Turn a display name into a stable, readable id.
///
/// Readability is the point: ids appear in every event's `actors`, and a log a
/// human can read is worth more than one with random identifiers in it.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut separator_pending = false;

    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator_pending && !out.is_empty() {
                out.push('-');
            }
            separator_pending = false;
            out.push(character.to_ascii_lowercase());
        } else {
            separator_pending = true;
        }
        if out.len() >= MAX_SLUG_LEN {
            break;
        }
    }

    // A name of only emoji or non-Latin script leaves nothing behind, and an
    // empty primary key is not a usable id.
    if out.is_empty() {
        "guest".to_string()
    } else {
        out
    }
}

/// Bring an existing `people` table up to the current shape.
///
/// Guarded by `PRAGMA table_info` rather than `ADD COLUMN IF NOT EXISTS`,
/// which the SQLite that Bookworm ships does not have.
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    let existing = column_names(conn)?;
    let has = |name: &str| existing.iter().any(|column| column == name);

    if !has("token") {
        conn.execute_batch("ALTER TABLE people ADD COLUMN token TEXT")?;
    }
    if !has("joined_ms") {
        // NOT NULL needs a default, and rows predating this migration have no
        // recorded join time. Zero says "before we started counting", which is
        // true, rather than inventing the moment the upgrade happened.
        conn.execute_batch("ALTER TABLE people ADD COLUMN joined_ms INTEGER NOT NULL DEFAULT 0")?;
    }
    // NULLs compare distinct in a SQLite unique index, so the pre-migration
    // rows that have no token do not collide with each other.
    conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_people_token ON people(token)")?;
    Ok(())
}

fn column_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(people)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn columns(store: &Store) -> Vec<String> {
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(people)").unwrap();
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        names
    }

    #[test]
    fn a_fresh_database_has_the_token_and_joined_ms_columns() {
        let names = columns(&store());
        assert!(names.contains(&"token".to_string()), "got {names:?}");
        assert!(names.contains(&"joined_ms".to_string()), "got {names:?}");
    }

    // The case that actually runs on the Pi: a database created by Arc 1,
    // with rows in it, opened by this build for the first time.
    #[test]
    fn an_arc1_database_is_migrated_without_losing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE people (id TEXT PRIMARY KEY, name TEXT NOT NULL, avatar TEXT);
                 INSERT INTO people (id, name) VALUES ('tate', 'Tate');",
            )
            .unwrap();
        }

        let store = Store::open(&path).unwrap();
        let names = columns(&store);
        assert!(names.contains(&"token".to_string()));
        assert!(names.contains(&"joined_ms".to_string()));

        let people = store.people().unwrap();
        assert_eq!(people.len(), 1, "the pre-existing row must survive");
        assert_eq!(people[0].name, "Tate");
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sesh.db");

        Store::open(&path)
            .unwrap()
            .insert_person("Sam", "t1")
            .unwrap();
        let reopened = Store::open(&path).unwrap();

        assert_eq!(reopened.people().unwrap().len(), 1);
    }

    #[test]
    fn inserting_a_person_allocates_a_slug_id_and_stamps_the_time() {
        let store = store();
        let person = store.insert_person("Marcus", "token-1").unwrap();

        assert_eq!(person.id, "marcus");
        assert_eq!(person.name, "Marcus");
        assert!(person.joined_ms > 0);
    }

    #[test]
    fn a_second_person_with_the_same_name_gets_a_distinct_id() {
        let store = store();
        let first = store.insert_person("Sam", "token-1").unwrap();
        let second = store.insert_person("Sam", "token-2").unwrap();

        assert_eq!(first.id, "sam");
        assert_eq!(second.id, "sam-2");
        assert_eq!(store.people().unwrap().len(), 2);
    }

    #[test]
    fn person_by_token_finds_the_holder() {
        let store = store();
        let written = store.insert_person("Sam", "secret").unwrap();

        assert_eq!(store.person_by_token("secret").unwrap(), Some(written));
    }

    #[test]
    fn person_by_token_is_none_for_an_unknown_token() {
        let store = store();
        store.insert_person("Sam", "secret").unwrap();

        assert_eq!(store.person_by_token("guessed").unwrap(), None);
    }

    // A token that reaches a client is a token that leaks. This is the guard
    // that keeps `Person` safe to hand to a handler.
    #[test]
    fn a_serialized_person_carries_no_token() {
        let store = store();
        let person = store.insert_person("Sam", "super-secret").unwrap();

        let json = serde_json::to_string(&person).unwrap();
        assert!(!json.contains("super-secret"), "token leaked into {json}");
        assert!(!json.contains("token"), "token field leaked into {json}");
    }

    #[test]
    fn person_by_id_finds_the_person() {
        let store = store();
        store.insert_person("Sam", "t1").unwrap();

        assert_eq!(store.person_by_id("sam").unwrap().unwrap().name, "Sam");
        assert_eq!(store.person_by_id("nobody").unwrap(), None);
    }

    #[test]
    fn people_lists_everyone_oldest_join_first() {
        let store = store();
        store.insert_person("Sam", "t1").unwrap();
        store.insert_person("Marcus", "t2").unwrap();

        let ids: Vec<_> = store.people().unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["sam".to_string(), "marcus".to_string()]);
    }

    #[test]
    fn slugify_lowercases_and_dashes_spaces() {
        assert_eq!(slugify("Big Marcus"), "big-marcus");
    }

    #[test]
    fn slugify_strips_punctuation_and_collapses_runs() {
        assert_eq!(slugify("Sam!!  O'Brien"), "sam-o-brien");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_separators() {
        assert_eq!(slugify("  -Tate-  "), "tate");
    }

    // A name that is entirely emoji or non-Latin script slugifies to nothing.
    // It must still produce a usable id rather than an empty primary key.
    #[test]
    fn slugify_falls_back_when_nothing_survives() {
        assert_eq!(slugify("🎉🎉"), "guest");
        assert_eq!(slugify(""), "guest");
    }

    // A display name is free-form text from a phone. The id derived from it
    // ends up in every event that person ever appears in.
    #[test]
    fn slugify_caps_the_length() {
        let slug = slugify(&"a".repeat(200));
        assert_eq!(slug.len(), MAX_SLUG_LEN);
    }
}
