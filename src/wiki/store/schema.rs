//! The store's schema and its versioned migrations.
//!
//! The schema version lives in `PRAGMA user_version`. Each migration is one
//! script that takes the database from version `n` to `n + 1`; they run in
//! order inside one transaction, so a failed upgrade leaves the file as it was.

use rusqlite::Connection;

use super::error::{Operation, StoreError};

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

const MIGRATIONS: [&str; 1] = [V1];

/// The newest schema version this build understands: the number of migrations.
pub(super) const LATEST: i64 = 1;

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
    for script in &MIGRATIONS[applied..] {
        transaction
            .execute_batch(script)
            .map_err(StoreError::database(Operation::Migrate))?;
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
}
