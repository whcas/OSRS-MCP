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

/// Version 3: a full-text index over title, section path and chunk text.
///
/// The index is contentless, so it stores terms only and the text lives once,
/// in `chunk`, and its row ids are the chunk's. Version 1 keyed chunks by a
/// composite key and so had no stable row id (`VACUUM` may renumber such
/// rows), so the chunk table is rebuilt with an `INTEGER PRIMARY KEY` first.
/// Triggers keep the index in step with `chunk`, so no write path can forget
/// it, and the last statement indexes the chunks already stored. A chunk is
/// indexed with the title its page has when the chunk is inserted: a page's
/// chunks are always replaced together with its page row.
const V3: &str = "
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

/// One step from schema version `n` to `n + 1`.
type Migration = fn(&Connection) -> rusqlite::Result<()>;

const MIGRATIONS: [Migration; 3] = [
    |connection| connection.execute_batch(V1),
    v2,
    |connection| connection.execute_batch(V3),
];

/// The newest schema version this build understands: the number of migrations.
pub(super) const LATEST: i64 = 3;

/// Tables a store at [`LATEST`] must have; `chunk_fts` is the search index.
const REQUIRED_TABLES: [&str; 4] = ["source", "page", "chunk", "chunk_fts"];

/// Checks that an existing database is at [`LATEST`] and has the store's
/// tables, without changing it.
///
/// # Errors
///
/// Fails if the version is newer or older than [`LATEST`] (version 0 is a file
/// that was never initialised), if a table is missing, or on a database error.
pub(super) fn validate(connection: &Connection) -> Result<(), StoreError> {
    let found: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StoreError::database(Operation::Read))?;
    if found > LATEST {
        return Err(StoreError::UnsupportedSchema {
            found,
            supported: LATEST,
        });
    }
    if found < LATEST {
        return Err(StoreError::NotCurrentSchema {
            found,
            required: LATEST,
        });
    }
    for table in REQUIRED_TABLES {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(StoreError::database(Operation::Read))?;
        if !exists {
            return Err(StoreError::Corrupt { what: "schema" });
        }
    }
    Ok(())
}

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

    /// The text each indexed word resolves to through the chunk table, which
    /// is what a search hit does.
    fn resolved(connection: &Connection, word: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT chunk.text FROM chunk_fts JOIN chunk ON chunk.id = chunk_fts.rowid
                 WHERE chunk_fts MATCH ?1 ORDER BY chunk.id",
            )
            .unwrap();
        statement
            .query_map([word], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn chunk_ids(connection: &Connection) -> Vec<(i64, String)> {
        let mut statement = connection
            .prepare("SELECT id, text FROM chunk ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn upgrading_a_populated_version_one_database_indexes_chunks_and_survives_vacuum() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(V1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute_batch(
                "INSERT INTO source VALUES (1, 'Wiki', '', 'https://w.example/');
                 INSERT INTO page VALUES (1, 7, 0, 'Axe', 3, '2026-01-02T03:04:05Z', 1, NULL, NULL, 'x');
                 INSERT INTO page VALUES (1, 8, 0, 'Pick', 4, '2026-01-02T03:04:05Z', 1, NULL, NULL, 'x');
                 INSERT INTO chunk VALUES (1, 7, 0, 0, 'Uses' || char(10) || 'Woodcutting',
                                           1, 2, 0, 9, 'prose', '', '', '', 'alpha chops');
                 INSERT INTO chunk VALUES (1, 7, 1, 0, 'Trivia', 1, 2, 9, 18, 'prose', '', '', '',
                                           'beta trees');
                 INSERT INTO chunk VALUES (1, 8, 0, 0, '', 1, 0, 0, 9, 'prose', '', '', '',
                                           'gamma rocks');
                 INSERT INTO chunk VALUES (1, 8, 0, 1, '', 1, 0, 9, 18, 'prose', '', '', '',
                                           'delta ore');",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        // Every chunk is indexed by its title, section path and text.
        assert_eq!(resolved(&connection, "alpha"), ["alpha chops"]);
        assert_eq!(resolved(&connection, "woodcutting"), ["alpha chops"]);
        assert_eq!(resolved(&connection, "pick"), ["gamma rocks", "delta ore"]);

        // The row ids the index relies on survive a rebuild of the file, which
        // is what the integer primary key is for: remove a chunk so ids have a
        // gap, then `VACUUM` and check that nothing was renumbered.
        connection
            .execute("DELETE FROM chunk WHERE text = 'alpha chops'", [])
            .unwrap();
        let before = chunk_ids(&connection);
        connection.execute_batch("VACUUM").unwrap();
        assert_eq!(chunk_ids(&connection), before);
        assert_eq!(resolved(&connection, "alpha"), Vec::<String>::new());
        for (word, text) in [
            ("beta", "beta trees"),
            ("gamma", "gamma rocks"),
            ("delta", "delta ore"),
        ] {
            assert_eq!(resolved(&connection, word), [text], "{word}");
        }
    }

    #[test]
    fn the_index_follows_chunk_inserts_and_deletes() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO source VALUES (1, 'Wiki', '', 'https://w.example/');
                 INSERT INTO page (source_id, page_id, namespace, title, revision_id, revision_time,
                                   revision_seconds, text)
                 VALUES (1, 7, 0, 'Axe', 3, '2026-01-02T03:04:05Z', 1, 'x');
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
