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

/// Version 2: a full-text index over title, section path and chunk text.
///
/// The index is contentless, so it stores terms only and the text lives once,
/// in `chunk`, and its row ids are the chunk's. Version 1 keyed chunks by a
/// composite key and so had no stable row id (`VACUUM` may renumber such
/// rows), so the chunk table is rebuilt with an `INTEGER PRIMARY KEY` first.
/// Triggers keep the index in step with `chunk`, so no write path can forget
/// it, and the last statement indexes the chunks already stored. A chunk is
/// indexed with the title its page has when the chunk is inserted: a page's
/// chunks are always replaced together with its page row.
const V2: &str = "
CREATE TABLE chunk_next (
    id INTEGER PRIMARY KEY,
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
    UNIQUE (source_id, page_id, section_index, chunk_index),
    FOREIGN KEY (source_id, page_id) REFERENCES page (source_id, page_id) ON DELETE CASCADE
) STRICT;

INSERT INTO chunk_next (source_id, page_id, section_index, chunk_index, section_path,
                        section_occurrence, section_level, source_start, source_end,
                        kind, losses, templates, link_targets, text)
SELECT source_id, page_id, section_index, chunk_index, section_path,
       section_occurrence, section_level, source_start, source_end,
       kind, losses, templates, link_targets, text
FROM chunk
ORDER BY source_id, page_id, section_index, chunk_index;

DROP TABLE chunk;
ALTER TABLE chunk_next RENAME TO chunk;

CREATE VIRTUAL TABLE chunk_fts USING fts5(
    title, section, body,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER chunk_indexed AFTER INSERT ON chunk BEGIN
    INSERT INTO chunk_fts (rowid, title, section, body)
    VALUES (
        new.id,
        (SELECT title FROM page WHERE source_id = new.source_id AND page_id = new.page_id),
        replace(new.section_path, char(10), ' '),
        new.text
    );
END;

CREATE TRIGGER chunk_unindexed AFTER DELETE ON chunk BEGIN
    DELETE FROM chunk_fts WHERE rowid = old.id;
END;

INSERT INTO chunk_fts (rowid, title, section, body)
SELECT chunk.id, page.title, replace(chunk.section_path, char(10), ' '), chunk.text
FROM chunk JOIN page ON page.source_id = chunk.source_id AND page.page_id = chunk.page_id;
";

const MIGRATIONS: [&str; 2] = [V1, V2];

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

    #[test]
    fn upgrading_from_version_one_keeps_chunks_and_indexes_them() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(V1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute_batch(
                "INSERT INTO source VALUES (1, 'Wiki', '', 'https://w.example/');
                 INSERT INTO page VALUES (1, 7, 0, 'Axe', 3, '2026-01-02T03:04:05Z', 1, NULL, NULL, 'x');
                 INSERT INTO chunk VALUES (1, 7, 0, 0, 'Uses' || char(10) || 'Woodcutting',
                                           1, 2, 0, 9, 'prose', '', '', '', 'chops needle');",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        let (id, text): (i64, String) = connection
            .query_row("SELECT id, text FROM chunk WHERE page_id = 7", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(text, "chops needle");
        for word in ["needle", "axe", "woodcutting"] {
            let matched: i64 = connection
                .query_row(
                    "SELECT rowid FROM chunk_fts WHERE chunk_fts MATCH ?1",
                    [word],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(matched, id, "{word}");
        }
    }

    #[test]
    fn the_index_follows_chunk_inserts_and_deletes() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO source VALUES (1, 'Wiki', '', 'https://w.example/');
                 INSERT INTO page VALUES (1, 7, 0, 'Axe', 3, '2026-01-02T03:04:05Z', 1, NULL, NULL, 'x');
                 INSERT INTO chunk (source_id, page_id, section_index, chunk_index, section_path,
                                    section_occurrence, section_level, source_start, source_end,
                                    kind, losses, templates, link_targets, text)
                 VALUES (1, 7, 0, 0, '', 1, 0, 0, 3, 'prose', '', '', '', 'needle');",
            )
            .unwrap();
        let indexed = |connection: &Connection| -> i64 {
            connection
                .query_row(
                    "SELECT count(*) FROM chunk_fts WHERE chunk_fts MATCH 'needle'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(indexed(&connection), 1);
        connection.execute("DELETE FROM chunk", []).unwrap();
        assert_eq!(indexed(&connection), 0);
    }
}
