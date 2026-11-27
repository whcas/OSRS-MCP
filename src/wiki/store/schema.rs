//! The store's schema and its versioned migrations.
//!
//! The schema version lives in `PRAGMA user_version`. Each migration is one
//! script that takes the database from version `n` to `n + 1`; they run in
//! order inside one transaction, so a failed upgrade leaves the file as it was.

use rusqlite::Connection;

use super::error::{Operation, StoreError};
use super::lookup::title_key;

/// Version 1: sources, current page revisions and their chunks.
///
/// Identifiers are `MediaWiki`'s, so a page is keyed by its source and page id.
/// Only the current revision of a page is kept. `db_name` is empty when the
/// export gave none, so that the source key can be unique.
const V1: &str = "
CREATE TABLE source (
    id INTEGER PRIMARY KEY,
    site_name TEXT NOT NULL,
    db_name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    UNIQUE (site_name, db_name, base_url)
) STRICT;

CREATE TABLE page (
    source_id INTEGER NOT NULL REFERENCES source (id),
    page_id INTEGER NOT NULL,
    namespace INTEGER NOT NULL,
    title TEXT NOT NULL,
    revision_id INTEGER NOT NULL,
    revision_time TEXT NOT NULL,
    revision_seconds INTEGER NOT NULL,
    hash TEXT,
    redirect_target TEXT,
    text TEXT NOT NULL,
    PRIMARY KEY (source_id, page_id)
) STRICT;

CREATE INDEX page_title ON page (source_id, title);

CREATE TABLE chunk (
    source_id INTEGER NOT NULL,
    page_id INTEGER NOT NULL,
    section_index INTEGER NOT NULL,
    chunk_index INTEGER NOT NULL,
    section_path TEXT NOT NULL,
    section_occurrence INTEGER NOT NULL,
    section_level INTEGER NOT NULL,
    source_start INTEGER NOT NULL,
    source_end INTEGER NOT NULL,
    kind TEXT NOT NULL,
    losses TEXT NOT NULL,
    templates TEXT NOT NULL,
    link_targets TEXT NOT NULL,
    text TEXT NOT NULL,
    PRIMARY KEY (source_id, page_id, section_index, chunk_index),
    FOREIGN KEY (source_id, page_id) REFERENCES page (source_id, page_id) ON DELETE CASCADE
) STRICT;
";

/// Version 2: the comparison form of each title, which lookups match on.
///
/// The raw `title` stays for display and citation. The form is computed in
/// Rust (see [`title_key`]), so existing rows are filled in by code, not SQL.
fn v2(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "ALTER TABLE page ADD COLUMN title_key TEXT NOT NULL DEFAULT '';
         CREATE INDEX page_title_key ON page (source_id, title_key);",
    )?;
    let titles = connection
        .prepare("SELECT source_id, page_id, title FROM page")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut update = connection
        .prepare("UPDATE page SET title_key = ?3 WHERE source_id = ?1 AND page_id = ?2")?;
    for (source_id, page_id, title) in titles {
        update.execute(rusqlite::params![source_id, page_id, title_key(&title)])?;
    }
    Ok(())
}

/// One step from schema version `n` to `n + 1`.
type Migration = fn(&Connection) -> rusqlite::Result<()>;

const MIGRATIONS: [Migration; 2] = [|connection| connection.execute_batch(V1), v2];

/// The newest schema version this build understands: the number of migrations.
pub(super) const LATEST: i64 = 2;

/// Brings the database to [`LATEST`], creating it if it is new.
///
/// # Errors
///
/// Fails if the file was written by a newer version, or on a database error.
pub(super) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection
        .transaction()
        .map_err(StoreError::database(Operation::Migrate))?;
    let found: i64 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StoreError::database(Operation::Migrate))?;
    if found > LATEST {
        return Err(StoreError::UnsupportedSchema {
            found,
            supported: LATEST,
        });
    }
    let applied = usize::try_from(found).map_err(|_| StoreError::Corrupt {
        what: "schema version",
    })?;
    for migration in &MIGRATIONS[applied..] {
        migration(&transaction).map_err(StoreError::database(Operation::Migrate))?;
    }
    transaction
        .pragma_update(None, "user_version", LATEST)
        .map_err(StoreError::database(Operation::Migrate))?;
    transaction
        .commit()
        .map_err(StoreError::database(Operation::Migrate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_version_is_the_number_of_migrations() {
        assert_eq!(usize::try_from(LATEST), Ok(MIGRATIONS.len()));
    }

    #[test]
    fn upgrading_from_version_one_fills_in_the_title_keys_of_stored_pages() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(V1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute_batch(
                "INSERT INTO source VALUES (1, 'Wiki', '', 'https://w.example/');
                 INSERT INTO page VALUES (1, 7, 0, 'dragon__axe#x', 3, '2026-01-02T03:04:05Z', 1,
                                          NULL, NULL, 'x');",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let (title, key): (String, String) = connection
            .query_row("SELECT title, title_key FROM page", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(title, "dragon__axe#x");
        assert_eq!(key, "Dragon axe");
    }
}
