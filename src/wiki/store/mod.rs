//! A local `SQLite` store for imported Wiki pages and their chunks.
//!
//! The store is the persistent end of the offline pipeline: pages come from
//! the importer, are prepared into chunks, and are kept with the provenance a
//! citation needs. It makes no network requests and runs nothing from page text.
//!
//! # Imports
//!
//! [`Store::begin_import`] opens an [`ImportSession`], which is one transaction.
//! Pages are added one at a time, so a whole export can be streamed without
//! being held in memory, and nothing is stored until [`ImportSession::commit`].
//! Dropping the session, or any failed `add`, leaves the database as it was
//! before the session began.
//!
//! For each page, in a session for one source:
//!
//! - a page the store does not have is **added**;
//! - a newer revision (a larger revision id) **replaces** the stored one,
//!   chunks included, in the same transaction;
//! - the same revision with the same title, namespace, hash and text is
//!   **unchanged**, which makes re-importing a file harmless;
//! - an older revision is [`StoreError::StaleRevision`] and the same revision
//!   with different content is [`StoreError::ConflictingRevision`]. The failed
//!   page is not stored, and what happens to the rest of the file is the
//!   caller's choice: dropping the session discards the whole import, which is
//!   the safe default for an export that is partly out of date.
//!
//! Pages missing from a later export are never deleted. Only the current
//! revision of a page is kept. The source is identified by the site name,
//! database name and base URL of the export header, which is unverified
//! provenance (see [`SourceId`]).
//!
//! # Lookup
//!
//! [`Store::page_by_title`] finds the page with an exact title and
//! [`Store::resolve`] follows redirects from it, reporting a missing target, a
//! loop or an ambiguous title as a [`Resolution`] rather than an empty result.
//! [`Store::search`] runs a bounded keyword search over chunk text, section
//! headings and titles; see [`SearchQuery`].

mod error;
mod lookup;
mod schema;
mod search;

use std::ops::Range;
use std::path::Path;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};

use super::{
    ChunkKind, ExportedHash, ImportedPage, Loss, PageId, PageTitle, PrepareLimits, PreparedPage,
    RevisionId, RevisionTimestamp, SourceId, prepare,
};
pub use error::{Operation, StoreError};
use lookup::title_key;
pub use lookup::{Resolution, TitleMatch};
pub use search::{
    InvalidSearch, MAX_QUERY_BYTES, MAX_QUERY_TERMS, MAX_RESULTS, SearchHit, SearchQuery,
};

/// Separates the entries of a list stored in one text column. None of the
/// stored names (headings, templates, link targets) can contain a line break.
const LIST_SEPARATOR: char = '\n';

/// A local database of imported Wiki pages.
pub struct Store {
    connection: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Opens the database at `path`, creating and migrating it if needed.
    ///
    /// The location is the caller's choice; nothing here picks a default.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened, was written by a newer version of
    /// this program, or cannot be migrated.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut connection =
            Connection::open(path).map_err(StoreError::database(Operation::Open))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(StoreError::database(Operation::Configure))?;
        schema::migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Opens an existing database for reading only.
    ///
    /// Unlike [`Self::open`] this never creates the file, never migrates it and
    /// never writes: an empty or outdated file is refused and left as it was.
    /// The connection is read-only, so [`Self::begin_import`] on the result
    /// fails with a database error.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened, is not at the current schema
    /// version (see [`StoreError::NotCurrentSchema`]), was written by a newer
    /// version of this program, or lacks the store's tables.
    pub fn open_existing(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(StoreError::database(Operation::Open))?;
        schema::validate(&connection)?;
        Ok(Self { connection })
    }

    /// Starts an import of pages from `source`.
    ///
    /// Takes the database's write lock until the session ends. `limits` bound
    /// the chunks made from each page.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn begin_import(
        &mut self,
        source: &SourceId,
        limits: PrepareLimits,
    ) -> Result<ImportSession<'_>, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::database(Operation::Write))?;
        let source_key = ensure_source(&transaction, source)?;
        Ok(ImportSession {
            transaction,
            source_key,
            limits,
            summary: ImportSummary::default(),
        })
    }

    /// The sources that have imported pages into this store, oldest first.
    ///
    /// # Errors
    ///
    /// Fails on a database error or if a stored value is invalid.
    pub fn sources(&self) -> Result<Vec<SourceId>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT site_name, db_name, base_url FROM source ORDER BY id")
            .map_err(StoreError::database(Operation::Read))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(StoreError::database(Operation::Read))?;
        rows.map(|row| {
            let (site_name, db_name, base_url) =
                row.map_err(StoreError::database(Operation::Read))?;
            // The empty string stands for "no database name" in storage.
            let db_name = (!db_name.is_empty()).then_some(db_name);
            SourceId::new(site_name, db_name, base_url)
                .map_err(|_| StoreError::Corrupt { what: "source" })
        })
        .collect()
    }

    /// The stored current revision of a page, or `None` if the store has none.
    ///
    /// # Errors
    ///
    /// Fails on a database error or if a stored value is invalid.
    pub fn page(&self, source: &SourceId, id: PageId) -> Result<Option<StoredPage>, StoreError> {
        let Some(source_key) = find_source(&self.connection, source)? else {
            return Ok(None);
        };
        read_page(&self.connection, source_key, id)
    }

    /// The stored chunks of a page in document order. Empty for an unknown page
    /// and for a redirect.
    ///
    /// # Errors
    ///
    /// Fails on a database error or if a stored value is invalid.
    pub fn chunks(&self, source: &SourceId, id: PageId) -> Result<Vec<StoredChunk>, StoreError> {
        let Some(source_key) = find_source(&self.connection, source)? else {
            return Ok(Vec::new());
        };
        read_chunks(&self.connection, source_key, id)
    }
}

/// What an import changed, counted by page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// Pages the store did not have.
    pub added: usize,
    /// Pages whose stored revision a newer one replaced.
    pub replaced: usize,
    /// Pages already stored at this revision with the same content.
    pub unchanged: usize,
}

/// What an import did with one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOutcome {
    /// The store did not have the page.
    Added,
    /// A newer revision replaced the stored one.
    Replaced,
    /// The same revision with the same content was already stored.
    Unchanged,
}

/// One transaction importing pages from one source. See the [module
/// documentation](self).
#[derive(Debug)]
pub struct ImportSession<'store> {
    transaction: Transaction<'store>,
    source_key: i64,
    limits: PrepareLimits,
    summary: ImportSummary,
}

impl ImportSession<'_> {
    /// Prepares and stores one page.
    ///
    /// # Errors
    ///
    /// Fails if the page cannot be prepared within the limits, if the store has
    /// a newer or conflicting revision of it, or on a database error. A failed
    /// page is not stored at all, even when the failure came after part of it
    /// was written: each page is written in a savepoint. The earlier pages of
    /// the session still are stored, until the session is dropped without a
    /// commit, and the session may go on.
    pub fn add(&mut self, page: &ImportedPage) -> Result<PageOutcome, StoreError> {
        let prepared = prepare(page, &self.limits).map_err(|source| StoreError::Prepare {
            page: page.id.get(),
            source,
        })?;
        let stored = stored_state(&self.transaction, self.source_key, page)?;
        let outcome = decide(stored, page)?;
        self.apply(outcome, page, &prepared)?;
        match outcome {
            PageOutcome::Added => self.summary.added += 1,
            PageOutcome::Replaced => self.summary.replaced += 1,
            PageOutcome::Unchanged => self.summary.unchanged += 1,
        }
        Ok(outcome)
    }

    /// Commits everything added so far.
    ///
    /// # Errors
    ///
    /// Fails on a database error, in which case nothing is stored.
    pub fn commit(self) -> Result<ImportSummary, StoreError> {
        self.transaction
            .commit()
            .map_err(StoreError::database(Operation::Commit))?;
        Ok(self.summary)
    }

    /// Writes one page inside a savepoint, so that a failure partway through
    /// (after the page row changed, or the old chunks were deleted) is undone
    /// and the session can still be committed without a half-written page.
    fn apply(
        &mut self,
        outcome: PageOutcome,
        page: &ImportedPage,
        prepared: &PreparedPage,
    ) -> Result<(), StoreError> {
        if outcome == PageOutcome::Unchanged {
            return Ok(());
        }
        let source_key = self.source_key;
        let savepoint = self
            .transaction
            .savepoint()
            .map_err(StoreError::database(Operation::Write))?;
        match outcome {
            PageOutcome::Unchanged => {}
            PageOutcome::Added => {
                insert_page(&savepoint, source_key, page)?;
                insert_chunks(&savepoint, source_key, prepared)?;
            }
            PageOutcome::Replaced => {
                update_page(&savepoint, source_key, page)?;
                delete_chunks(&savepoint, source_key, page.id)?;
                insert_chunks(&savepoint, source_key, prepared)?;
            }
        }
        savepoint
            .commit()
            .map_err(StoreError::database(Operation::Write))
    }
}

/// What the store holds for a page that an offered revision is compared with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredState {
    revision: u64,
    /// Whether title, namespace, hash and text equal the offered page's.
    same_content: bool,
}

/// Decides what to do with an offered page given what is stored. Pure.
fn decide(stored: Option<StoredState>, page: &ImportedPage) -> Result<PageOutcome, StoreError> {
    let offered = page.revision.get();
    let Some(stored) = stored else {
        return Ok(PageOutcome::Added);
    };
    match offered.cmp(&stored.revision) {
        std::cmp::Ordering::Greater => Ok(PageOutcome::Replaced),
        std::cmp::Ordering::Equal if stored.same_content => Ok(PageOutcome::Unchanged),
        std::cmp::Ordering::Equal => Err(StoreError::ConflictingRevision {
            page: page.id.get(),
            revision: offered,
        }),
        std::cmp::Ordering::Less => Err(StoreError::StaleRevision {
            page: page.id.get(),
            stored: stored.revision,
            offered,
        }),
    }
}

/// A page as the store holds it: its current revision.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredPage {
    /// The page identifier.
    pub id: PageId,
    /// The numeric `MediaWiki` namespace.
    pub namespace: u32,
    /// The page title.
    pub title: PageTitle,
    /// The stored revision.
    pub revision: RevisionId,
    /// When the revision was saved, as exported.
    pub timestamp: RevisionTimestamp,
    /// The hash the export reported, if any.
    pub hash: Option<String>,
    /// The redirect target the export declared, if the page is a redirect.
    pub redirect_target: Option<PageTitle>,
    /// The raw wikitext of the revision.
    pub text: String,
}

impl std::fmt::Debug for StoredPage {
    /// Omits titles and text, like [`ImportedPage`].
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredPage")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// One stored chunk, with the section it came from.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredChunk {
    /// Position of the chunk's section among the page's sections.
    pub section: usize,
    /// The section's heading titles, outermost first. Empty for the lead.
    pub section_path: Vec<String>,
    /// Which of several sections with this path this is, from zero.
    pub section_occurrence: u32,
    /// The heading level, 1 to 6, or 0 for the lead.
    pub section_level: u8,
    /// Position of the chunk among its section's chunks.
    pub index: usize,
    /// The byte range of the chunk in the page's wikitext.
    pub source: Range<usize>,
    /// Whether the chunk is more than template calls.
    pub kind: ChunkKind,
    /// What the chunk's edges damage.
    pub losses: Vec<Loss>,
    /// The unexpanded templates the chunk calls.
    pub templates: Vec<String>,
    /// The pages the chunk links to.
    pub link_targets: Vec<String>,
    /// The chunk's wikitext.
    pub text: String,
}

impl std::fmt::Debug for StoredChunk {
    /// Omits the text, headings and names, like [`Chunk`](super::Chunk).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredChunk")
            .field("section", &self.section)
            .field("index", &self.index)
            .field("source", &self.source)
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

fn id_to_sql(id: u64) -> Result<i64, StoreError> {
    i64::try_from(id).map_err(|_| StoreError::IdentifierTooLarge)
}

fn position_to_sql(position: usize) -> Result<i64, StoreError> {
    i64::try_from(position).map_err(|_| StoreError::IdentifierTooLarge)
}

fn ensure_source(transaction: &Transaction<'_>, source: &SourceId) -> Result<i64, StoreError> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO source (site_name, db_name, base_url) VALUES (?1, ?2, ?3)",
            params![
                source.site_name(),
                source.db_name().unwrap_or_default(),
                source.base_url()
            ],
        )
        .map_err(StoreError::database(Operation::Write))?;
    find_source(transaction, source)?.ok_or(StoreError::Corrupt { what: "source" })
}

fn find_source(connection: &Connection, source: &SourceId) -> Result<Option<i64>, StoreError> {
    connection
        .query_row(
            "SELECT id FROM source WHERE site_name = ?1 AND db_name = ?2 AND base_url = ?3",
            params![
                source.site_name(),
                source.db_name().unwrap_or_default(),
                source.base_url()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::database(Operation::Read))
}

fn stored_state(
    transaction: &Transaction<'_>,
    source_key: i64,
    page: &ImportedPage,
) -> Result<Option<StoredState>, StoreError> {
    let row: Option<(i64, bool)> = transaction
        .query_row(
            "SELECT revision_id,
                    title = ?3 AND namespace = ?4 AND hash IS ?5 AND text = ?6
             FROM page WHERE source_id = ?1 AND page_id = ?2",
            params![
                source_key,
                id_to_sql(page.id.get())?,
                page.title.as_str(),
                page.namespace,
                page.hash.as_ref().map(ExportedHash::as_str),
                page.text,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(StoreError::database(Operation::Read))?;
    row.map(|(revision, same_content)| {
        Ok(StoredState {
            revision: u64::try_from(revision).map_err(|_| StoreError::Corrupt {
                what: "revision id",
            })?,
            same_content,
        })
    })
    .transpose()
}

fn insert_page(
    connection: &Connection,
    source_key: i64,
    page: &ImportedPage,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO page (source_id, page_id, namespace, title, revision_id,
                               revision_time, revision_seconds, hash, redirect_target, text,
                               title_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            page_params(source_key, page)?,
        )
        .map_err(StoreError::database(Operation::Write))?;
    Ok(())
}

fn update_page(
    connection: &Connection,
    source_key: i64,
    page: &ImportedPage,
) -> Result<(), StoreError> {
    connection
        .execute(
            "UPDATE page SET namespace = ?3, title = ?4, revision_id = ?5, revision_time = ?6,
                             revision_seconds = ?7, hash = ?8, redirect_target = ?9, text = ?10,
                             title_key = ?11
             WHERE source_id = ?1 AND page_id = ?2",
            page_params(source_key, page)?,
        )
        .map_err(StoreError::database(Operation::Write))?;
    Ok(())
}

type PageParams<'page> = (
    i64,
    i64,
    u32,
    &'page str,
    i64,
    &'page str,
    i64,
    Option<&'page str>,
    Option<&'page str>,
    &'page str,
    String,
);

fn page_params(source_key: i64, page: &ImportedPage) -> Result<PageParams<'_>, StoreError> {
    Ok((
        source_key,
        id_to_sql(page.id.get())?,
        page.namespace,
        page.title.as_str(),
        id_to_sql(page.revision.get())?,
        page.timestamp.as_str(),
        id_to_sql(page.timestamp.seconds().0)?,
        page.hash.as_ref().map(ExportedHash::as_str),
        page.redirect_target.as_ref().map(PageTitle::as_str),
        &page.text,
        title_key(page.title.as_str()),
    ))
}

fn delete_chunks(connection: &Connection, source_key: i64, page: PageId) -> Result<(), StoreError> {
    connection
        .execute(
            "DELETE FROM chunk WHERE source_id = ?1 AND page_id = ?2",
            params![source_key, id_to_sql(page.get())?],
        )
        .map_err(StoreError::database(Operation::Write))?;
    Ok(())
}

fn insert_chunks(
    connection: &Connection,
    source_key: i64,
    prepared: &PreparedPage,
) -> Result<(), StoreError> {
    let write = StoreError::database(Operation::Write);
    let mut statement = connection
        .prepare(
            "INSERT INTO chunk (source_id, page_id, section_index, chunk_index, section_path,
                                section_occurrence, section_level, source_start, source_end,
                                kind, losses, templates, link_targets, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )
        .map_err(write)?;
    let page = id_to_sql(prepared.page.get())?;
    for chunk in &prepared.chunks {
        let section = prepared
            .section_of(chunk)
            .ok_or(StoreError::Corrupt { what: "section" })?;
        statement
            .execute(params![
                source_key,
                page,
                position_to_sql(chunk.section)?,
                position_to_sql(chunk.index)?,
                join_list(section.id.path.titles()),
                section.id.occurrence,
                section.level,
                position_to_sql(chunk.source.start)?,
                position_to_sql(chunk.source.end)?,
                kind_name(chunk.kind),
                chunk
                    .losses
                    .iter()
                    .map(|loss| loss_name(*loss))
                    .collect::<Vec<_>>()
                    .join(","),
                join_list(&chunk.templates),
                join_list(&chunk.link_targets),
                chunk.text,
            ])
            .map_err(StoreError::database(Operation::Write))?;
    }
    Ok(())
}

fn join_list(entries: &[String]) -> String {
    entries.join(&LIST_SEPARATOR.to_string())
}

fn split_list(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split(LIST_SEPARATOR).map(str::to_owned).collect()
    }
}

const fn kind_name(kind: ChunkKind) -> &'static str {
    match kind {
        ChunkKind::Prose => "prose",
        ChunkKind::TemplateOnly => "template_only",
    }
}

fn parse_kind(name: &str) -> Result<ChunkKind, StoreError> {
    match name {
        "prose" => Ok(ChunkKind::Prose),
        "template_only" => Ok(ChunkKind::TemplateOnly),
        _ => Err(StoreError::Corrupt { what: "chunk kind" }),
    }
}

const fn loss_name(loss: Loss) -> &'static str {
    match loss {
        Loss::CutLine => "cut_line",
        Loss::SplitMarkup => "split_markup",
        Loss::SplitTable => "split_table",
        Loss::SplitCaveat => "split_caveat",
    }
}

fn parse_losses(text: &str) -> Result<Vec<Loss>, StoreError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|name| match name {
            "cut_line" => Ok(Loss::CutLine),
            "split_markup" => Ok(Loss::SplitMarkup),
            "split_table" => Ok(Loss::SplitTable),
            "split_caveat" => Ok(Loss::SplitCaveat),
            _ => Err(StoreError::Corrupt { what: "chunk loss" }),
        })
        .collect()
}

type PageRow = (
    i64,
    i64,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    String,
);

fn read_page(
    connection: &Connection,
    source_key: i64,
    id: PageId,
) -> Result<Option<StoredPage>, StoreError> {
    let row: Option<PageRow> = connection
        .query_row(
            "SELECT page_id, namespace, title, revision_id, revision_time, hash,
                    redirect_target, text
             FROM page WHERE source_id = ?1 AND page_id = ?2",
            params![source_key, id_to_sql(id.get())?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::database(Operation::Read))?;
    row.map(stored_page).transpose()
}

fn stored_page(row: PageRow) -> Result<StoredPage, StoreError> {
    let (page_id, namespace, title, revision, time, hash, redirect, text) = row;
    let invalid = |what| StoreError::Corrupt { what };
    Ok(StoredPage {
        id: u64::try_from(page_id)
            .ok()
            .and_then(PageId::new)
            .ok_or_else(|| invalid("page id"))?,
        namespace: u32::try_from(namespace).map_err(|_| invalid("namespace"))?,
        title: PageTitle::new(title).ok_or_else(|| invalid("title"))?,
        revision: u64::try_from(revision)
            .ok()
            .and_then(RevisionId::new)
            .ok_or_else(|| invalid("revision id"))?,
        timestamp: RevisionTimestamp::parse(&time).map_err(|_| invalid("revision time"))?,
        hash,
        redirect_target: redirect
            .map(|target| PageTitle::new(target).ok_or_else(|| invalid("redirect target")))
            .transpose()?,
        text,
    })
}

type ChunkRow = (
    i64,
    i64,
    String,
    i64,
    i64,
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
);

fn read_chunks(
    connection: &Connection,
    source_key: i64,
    id: PageId,
) -> Result<Vec<StoredChunk>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT section_index, chunk_index, section_path, section_occurrence, section_level,
                    source_start, source_end, kind, losses, templates, link_targets, text
             FROM chunk WHERE source_id = ?1 AND page_id = ?2
             ORDER BY section_index, chunk_index",
        )
        .map_err(StoreError::database(Operation::Read))?;
    let rows = statement
        .query_map(params![source_key, id_to_sql(id.get())?], |row| {
            chunk_row(row, 0)
        })
        .map_err(StoreError::database(Operation::Read))?;
    rows.map(|row| stored_chunk(row.map_err(StoreError::database(Operation::Read))?))
        .collect()
}

/// Reads the twelve chunk columns in the order of [`ChunkRow`], starting at
/// column `first`.
fn chunk_row(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<ChunkRow> {
    Ok((
        row.get(first)?,
        row.get(first + 1)?,
        row.get(first + 2)?,
        row.get(first + 3)?,
        row.get(first + 4)?,
        row.get(first + 5)?,
        row.get(first + 6)?,
        row.get(first + 7)?,
        row.get(first + 8)?,
        row.get(first + 9)?,
        row.get(first + 10)?,
        row.get(first + 11)?,
    ))
}

fn stored_chunk(row: ChunkRow) -> Result<StoredChunk, StoreError> {
    let (section, index, path, occurrence, level, start, end, kind, losses, templates, links, text) =
        row;
    let invalid = |what| StoreError::Corrupt { what };
    Ok(StoredChunk {
        section: usize::try_from(section).map_err(|_| invalid("section index"))?,
        section_path: split_list(&path),
        section_occurrence: u32::try_from(occurrence).map_err(|_| invalid("section occurrence"))?,
        section_level: u8::try_from(level).map_err(|_| invalid("section level"))?,
        index: usize::try_from(index).map_err(|_| invalid("chunk index"))?,
        source: usize::try_from(start).map_err(|_| invalid("chunk range"))?
            ..usize::try_from(end).map_err(|_| invalid("chunk range"))?,
        kind: parse_kind(&kind)?,
        losses: parse_losses(&losses)?,
        templates: split_list(&templates),
        link_targets: split_list(&links),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    pub(super) struct Fixture {
        directory: TempDir,
        pub(super) source: SourceId,
    }

    impl Fixture {
        pub(super) fn new() -> Self {
            Self {
                directory: TempDir::new().unwrap(),
                source: SourceId::new(
                    "Test Wiki".to_owned(),
                    Some("testwiki".to_owned()),
                    "https://wiki.example/wiki/Main_Page".to_owned(),
                )
                .unwrap(),
            }
        }

        fn path(&self) -> std::path::PathBuf {
            self.directory.path().join("wiki.sqlite")
        }

        pub(super) fn open(&self) -> Store {
            Store::open(&self.path()).unwrap()
        }
    }

    pub(super) fn page(id: u64, revision: u64, text: &str) -> ImportedPage {
        ImportedPage {
            id: PageId::new(id).unwrap(),
            namespace: 0,
            title: PageTitle::new(format!("Page {id}")).unwrap(),
            revision: RevisionId::new(revision).unwrap(),
            timestamp: RevisionTimestamp::parse("2026-01-02T03:04:05Z").unwrap(),
            hash: ExportedHash::new(format!("hash{revision}")),
            redirect_target: None,
            text: text.to_owned(),
        }
    }

    pub(super) fn import(
        store: &mut Store,
        source: &SourceId,
        pages: &[ImportedPage],
    ) -> ImportSummary {
        let mut session = store
            .begin_import(source, PrepareLimits::default())
            .unwrap();
        for page in pages {
            session.add(page).unwrap();
        }
        session.commit().unwrap()
    }

    pub(super) fn page_id(id: u64) -> PageId {
        PageId::new(id).unwrap()
    }

    #[test]
    fn open_existing_reads_a_current_database_and_refuses_to_import_into_it() {
        let fixture = Fixture::new();
        import(&mut fixture.open(), &fixture.source, &[page(1, 1, "Text.")]);
        let mut store = Store::open_existing(&fixture.path()).unwrap();
        assert_eq!(revision_of(&store, &fixture.source, 1), Some(1));
        assert!(matches!(
            store.begin_import(&fixture.source, PrepareLimits::default()),
            Err(StoreError::Database {
                operation: Operation::Write,
                ..
            })
        ));
    }

    #[test]
    fn open_existing_refuses_an_empty_file_and_leaves_it_empty() {
        let fixture = Fixture::new();
        let path = fixture.path();
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(
            Store::open_existing(&path),
            Err(StoreError::NotCurrentSchema {
                found: 0,
                required: 3
            })
        ));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    }

    #[test]
    fn open_existing_refuses_an_outdated_database_without_upgrading_it() {
        let fixture = Fixture::new();
        drop(fixture.open());
        let path = fixture.path();
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 2)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            Store::open_existing(&path),
            Err(StoreError::NotCurrentSchema {
                found: 2,
                required: 3
            })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn open_existing_refuses_a_newer_database() {
        let fixture = Fixture::new();
        drop(fixture.open());
        Connection::open(fixture.path())
            .unwrap()
            .pragma_update(None, "user_version", 4)
            .unwrap();
        assert!(matches!(
            Store::open_existing(&fixture.path()),
            Err(StoreError::UnsupportedSchema { found: 4, .. })
        ));
    }

    #[test]
    fn open_existing_refuses_a_database_without_the_stores_tables() {
        let fixture = Fixture::new();
        let connection = Connection::open(fixture.path()).unwrap();
        connection.pragma_update(None, "user_version", 3).unwrap();
        drop(connection);
        assert!(matches!(
            Store::open_existing(&fixture.path()),
            Err(StoreError::Corrupt { what: "schema" })
        ));
    }

    #[test]
    fn open_existing_does_not_create_a_missing_file() {
        let fixture = Fixture::new();
        assert!(matches!(
            Store::open_existing(&fixture.path()),
            Err(StoreError::Database {
                operation: Operation::Open,
                ..
            })
        ));
        assert!(!fixture.path().exists());
    }

    fn revision_of(store: &Store, source: &SourceId, id: u64) -> Option<u64> {
        store
            .page(source, page_id(id))
            .unwrap()
            .map(|stored| stored.revision.get())
    }

    fn texts(store: &Store, source: &SourceId, id: u64) -> Vec<String> {
        store
            .chunks(source, page_id(id))
            .unwrap()
            .into_iter()
            .map(|chunk| chunk.text)
            .collect()
    }

    #[test]
    fn a_new_database_is_created_at_the_latest_schema_version() {
        let fixture = Fixture::new();
        drop(fixture.open());
        let connection = Connection::open(fixture.path()).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, schema::LATEST);
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused_and_left_alone() {
        let fixture = Fixture::new();
        drop(fixture.open());
        Connection::open(fixture.path())
            .unwrap()
            .pragma_update(None, "user_version", schema::LATEST + 1)
            .unwrap();
        let error = Store::open(&fixture.path()).unwrap_err();
        assert!(matches!(
            error,
            StoreError::UnsupportedSchema { found, supported }
                if found == schema::LATEST + 1 && supported == schema::LATEST
        ));
    }

    #[test]
    fn opening_an_unusable_path_is_a_database_error_without_the_path() {
        let fixture = Fixture::new();
        let missing = fixture.directory.path().join("absent").join("wiki.sqlite");
        let error = Store::open(&missing).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Database {
                operation: Operation::Open,
                ..
            }
        ));
        assert!(!error.to_string().contains("absent"));
    }

    #[test]
    fn an_added_page_keeps_its_provenance_and_chunks() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let mut imported = page(7, 70, "Lead {{T|x}}\n== A ==\nbody [[Link]]");
        imported.redirect_target = None;
        let summary = import(&mut store, &fixture.source, std::slice::from_ref(&imported));
        assert_eq!(
            summary,
            ImportSummary {
                added: 1,
                replaced: 0,
                unchanged: 0
            }
        );

        let stored = store.page(&fixture.source, imported.id).unwrap().unwrap();
        assert_eq!(stored.title, imported.title);
        assert_eq!(stored.revision, imported.revision);
        assert_eq!(stored.timestamp, imported.timestamp);
        assert_eq!(stored.hash.as_deref(), Some("hash70"));
        assert_eq!(stored.text, imported.text);

        let chunks = store.chunks(&fixture.source, imported.id).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].section_path, Vec::<String>::new());
        assert_eq!(chunks[0].templates, ["T"]);
        assert_eq!(chunks[1].section_path, ["A"]);
        assert_eq!(chunks[1].section_level, 2);
        assert_eq!(chunks[1].link_targets, ["Link"]);
        assert_eq!(chunks[1].text, "body [[Link]]");
        assert_eq!(&imported.text[chunks[1].source.clone()], chunks[1].text);
    }

    #[test]
    fn a_redirect_is_stored_with_its_target_and_no_chunks() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let mut redirect = page(8, 80, "#REDIRECT [[Target]]");
        redirect.redirect_target = PageTitle::new("Target".to_owned());
        import(&mut store, &fixture.source, std::slice::from_ref(&redirect));
        let stored = store.page(&fixture.source, redirect.id).unwrap().unwrap();
        assert_eq!(stored.redirect_target, redirect.redirect_target);
        assert_eq!(store.chunks(&fixture.source, redirect.id).unwrap().len(), 0);
    }

    #[test]
    fn importing_the_same_pages_again_changes_nothing() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let pages = [page(1, 10, "one"), page(2, 20, "two\n\nmore")];
        import(&mut store, &fixture.source, &pages);
        let before = texts(&store, &fixture.source, 2);
        let summary = import(&mut store, &fixture.source, &pages);
        assert_eq!(
            summary,
            ImportSummary {
                added: 0,
                replaced: 0,
                unchanged: 2
            }
        );
        assert_eq!(texts(&store, &fixture.source, 2), before);
    }

    #[test]
    fn a_newer_revision_replaces_the_page_and_its_chunks_together() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(
            &mut store,
            &fixture.source,
            &[page(1, 10, "old\n== A ==\nold a")],
        );
        let summary = import(&mut store, &fixture.source, &[page(1, 11, "new only")]);
        assert_eq!(summary.replaced, 1);
        assert_eq!(revision_of(&store, &fixture.source, 1), Some(11));
        assert_eq!(texts(&store, &fixture.source, 1), ["new only"]);
    }

    #[test]
    fn an_older_revision_is_rejected_and_the_stored_page_is_untouched() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(&mut store, &fixture.source, &[page(1, 11, "newer")]);
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        let error = session.add(&page(1, 10, "older")).unwrap_err();
        assert!(matches!(
            error,
            StoreError::StaleRevision {
                page: 1,
                stored: 11,
                offered: 10
            }
        ));
        drop(session);
        assert_eq!(texts(&store, &fixture.source, 1), ["newer"]);
    }

    #[test]
    fn the_same_revision_with_different_content_is_a_conflict() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(&mut store, &fixture.source, &[page(1, 10, "original")]);
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        let error = session.add(&page(1, 10, "altered")).unwrap_err();
        assert!(matches!(
            error,
            StoreError::ConflictingRevision {
                page: 1,
                revision: 10
            }
        ));
        let mut other_hash = page(1, 10, "original");
        other_hash.hash = ExportedHash::new("different".to_owned());
        assert!(session.add(&other_hash).is_err());
    }

    #[test]
    fn a_session_dropped_without_commit_stores_nothing() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        session.add(&page(1, 10, "text")).unwrap();
        drop(session);
        assert_eq!(revision_of(&store, &fixture.source, 1), None);
        assert_eq!(
            store
                .chunks(&fixture.source, PageId::new(1).unwrap())
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_failed_page_leaves_the_session_uncommitted_so_earlier_pages_roll_back() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(&mut store, &fixture.source, &[page(2, 20, "stored")]);
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        session.add(&page(1, 10, "first")).unwrap();
        assert!(session.add(&page(2, 19, "stale")).is_err());
        drop(session);
        assert_eq!(revision_of(&store, &fixture.source, 1), None);
        assert_eq!(revision_of(&store, &fixture.source, 2), Some(20));
    }

    /// Makes the insert of any chunk whose text is `boom` fail, which happens
    /// after the page row has been written or replaced.
    fn fail_chunk_writes_of_boom(store: &Store) {
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER forced_failure BEFORE INSERT ON chunk WHEN new.text = 'boom'
                 BEGIN SELECT RAISE(ABORT, 'forced'); END;",
            )
            .unwrap();
    }

    #[test]
    fn a_chunk_write_failing_after_the_page_write_stores_nothing_of_that_page() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        fail_chunk_writes_of_boom(&store);
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        session.add(&page(1, 10, "first")).unwrap();
        let failure = session.add(&page(2, 20, "boom")).unwrap_err();
        assert!(matches!(
            failure,
            StoreError::Database {
                operation: Operation::Write,
                ..
            }
        ));
        session.add(&page(3, 30, "third")).unwrap();
        let summary = session.commit().unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(revision_of(&store, &fixture.source, 2), None);
        assert_eq!(texts(&store, &fixture.source, 2).len(), 0);
        assert_eq!(texts(&store, &fixture.source, 1), ["first"]);
        assert_eq!(texts(&store, &fixture.source, 3), ["third"]);
    }

    #[test]
    fn a_failed_replacement_keeps_the_old_page_and_chunks_after_commit() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(&mut store, &fixture.source, &[page(5, 50, "old")]);
        fail_chunk_writes_of_boom(&store);
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        assert!(session.add(&page(5, 60, "boom")).is_err());
        let summary = session.commit().unwrap();
        assert_eq!(summary.replaced, 0);
        assert_eq!(revision_of(&store, &fixture.source, 5), Some(50));
        assert_eq!(texts(&store, &fixture.source, 5), ["old"]);
    }

    #[test]
    fn pages_absent_from_a_later_import_are_kept() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(
            &mut store,
            &fixture.source,
            &[page(1, 10, "one"), page(2, 20, "two")],
        );
        import(&mut store, &fixture.source, &[page(1, 11, "one again")]);
        assert_eq!(revision_of(&store, &fixture.source, 2), Some(20));
        assert_eq!(texts(&store, &fixture.source, 2), ["two"]);
    }

    #[test]
    fn stored_pages_survive_closing_and_reopening_the_database() {
        let fixture = Fixture::new();
        {
            let mut store = fixture.open();
            import(
                &mut store,
                &fixture.source,
                &[page(1, 10, "kept\n== A ==\nalso kept")],
            );
        }
        let store = fixture.open();
        assert_eq!(revision_of(&store, &fixture.source, 1), Some(10));
        assert_eq!(texts(&store, &fixture.source, 1), ["kept", "also kept"]);
    }

    #[test]
    fn the_same_page_id_from_another_source_is_a_different_page() {
        let fixture = Fixture::new();
        let other = SourceId::new(
            "Other Wiki".to_owned(),
            None,
            "https://other.example/wiki/Main_Page".to_owned(),
        )
        .unwrap();
        let mut store = fixture.open();
        import(&mut store, &fixture.source, &[page(1, 10, "mine")]);
        import(&mut store, &other, &[page(1, 5, "theirs")]);
        assert_eq!(texts(&store, &fixture.source, 1), ["mine"]);
        assert_eq!(texts(&store, &other, 1), ["theirs"]);
        assert_eq!(revision_of(&store, &other, 1), Some(5));
    }

    #[test]
    fn sources_lists_every_source_that_imported_pages_in_order() {
        let fixture = Fixture::new();
        let other = SourceId::new(
            "Other Wiki".to_owned(),
            None,
            "https://other.example/wiki/Main_Page".to_owned(),
        )
        .unwrap();
        let mut store = fixture.open();
        assert_eq!(store.sources().unwrap(), []);
        import(&mut store, &fixture.source, &[page(1, 10, "mine")]);
        import(&mut store, &other, &[page(1, 5, "theirs")]);
        assert_eq!(store.sources().unwrap(), [fixture.source.clone(), other]);
    }

    #[test]
    fn an_unknown_source_or_page_reads_as_absent() {
        let fixture = Fixture::new();
        let store = fixture.open();
        assert_eq!(revision_of(&store, &fixture.source, 1), None);
        assert_eq!(texts(&store, &fixture.source, 1).len(), 0);
    }

    #[test]
    fn a_page_that_cannot_be_prepared_is_a_typed_error_naming_only_its_id() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let limits = PrepareLimits::new(100, 2).unwrap();
        let mut session = store.begin_import(&fixture.source, limits).unwrap();
        let error = session
            .add(&page(3, 30, "= a =\n= b =\n= c =\nSECRETMARKER"))
            .unwrap_err();
        assert!(matches!(error, StoreError::Prepare { page: 3, .. }));
        assert!(!format!("{error:?} {error}").contains("SECRETMARKER"));
    }

    #[test]
    fn identifiers_beyond_the_database_range_are_rejected() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let mut session = store
            .begin_import(&fixture.source, PrepareLimits::default())
            .unwrap();
        let error = session.add(&page(u64::MAX, 1, "x")).unwrap_err();
        assert!(matches!(error, StoreError::IdentifierTooLarge));
    }

    #[test]
    fn debug_output_omits_page_and_chunk_text() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let imported = page(1, 10, "SECRETMARKER");
        import(&mut store, &fixture.source, std::slice::from_ref(&imported));
        let stored = store.page(&fixture.source, imported.id).unwrap().unwrap();
        let chunks = store.chunks(&fixture.source, imported.id).unwrap();
        let debug = format!("{stored:?} {chunks:?} {store:?}");
        assert!(!debug.contains("SECRETMARKER"), "{debug}");
        assert!(!debug.contains("Page 1"), "{debug}");
    }

    #[test]
    fn a_stored_value_the_program_could_not_have_written_is_reported_as_corrupt() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        import(&mut store, &fixture.source, &[page(1, 10, "text")]);
        drop(store);
        Connection::open(fixture.path())
            .unwrap()
            .execute("UPDATE chunk SET kind = 'bogus'", [])
            .unwrap();
        let store = fixture.open();
        let error = store
            .chunks(&fixture.source, PageId::new(1).unwrap())
            .unwrap_err();
        assert!(matches!(error, StoreError::Corrupt { what: "chunk kind" }));
    }
}
