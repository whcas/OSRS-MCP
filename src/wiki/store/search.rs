//! Bounded keyword search over the stored chunks.
//!
//! Search runs on a full-text index of each chunk's page title, section path
//! and **wikitext** (not rendered text), so template and parameter names in
//! the markup are searchable too; [`ChunkKind`](crate::wiki::ChunkKind) on a
//! hit says when a chunk is mostly unexpanded templates. Results come from the
//! requested source only and are never mixed with another's.
//!
//! The user's words are never passed to the index as query syntax: each one
//! becomes a quoted term, so operators, column filters and prefix marks in the
//! input are searched for as words, and all terms must match (in any order).
//! Order is by the index's `bm25` relevance with equal column weights, then by
//! page id, section index and chunk index, so the same database and query
//! always give the same list. The equal weights are a starting point to be
//! tuned against the evaluation dataset, not a measured choice.
//!
//! **Ranking scope.** The `source` argument filters which hits are returned,
//! but the index is shared by every source in the database, so its `bm25`
//! statistics (term rarity and average length) are corpus-wide. Importing
//! another source can change the relative order of hits in this one. To compare
//! rankings, as the evaluation runner does, use a database that holds only the
//! source under test, and keep it fixed between runs.

use rusqlite::params;
use thiserror::Error;

use super::{
    Operation, PageId, PageTitle, RevisionId, RevisionTimestamp, SourceId, Store, StoreError,
    StoredChunk, chunk_row, find_source, stored_chunk,
};

/// The longest query text accepted, in bytes.
pub const MAX_QUERY_BYTES: usize = 512;
/// The most words one query may have.
pub const MAX_QUERY_TERMS: usize = 16;
/// The most hits one search may ask for.
pub const MAX_RESULTS: usize = 50;

/// Why a search was refused before it ran. Never carries the query text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InvalidSearch {
    /// The query has no words.
    #[error("the search query has no words")]
    Empty,
    /// The query is longer than [`MAX_QUERY_BYTES`].
    #[error("the search query is longer than {MAX_QUERY_BYTES} bytes")]
    TooLong,
    /// The query has more than [`MAX_QUERY_TERMS`] words.
    #[error("the search query has more than {MAX_QUERY_TERMS} words")]
    TooManyTerms,
    /// A limit of zero asks for nothing.
    #[error("the result limit is zero")]
    ZeroLimit,
    /// The limit is above [`MAX_RESULTS`].
    #[error("the result limit is above {MAX_RESULTS}")]
    LimitTooLarge,
}

/// A validated keyword search: its words and how many hits to return.
#[derive(Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// The words, each quoted, ready to hand to the index.
    expression: String,
    limit: usize,
}

impl std::fmt::Debug for SearchQuery {
    /// Shows the limit only: the words are user input.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SearchQuery")
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl SearchQuery {
    /// Builds a search for the words of `text`, returning at most `limit`
    /// hits. Whitespace and control characters separate words.
    ///
    /// # Errors
    ///
    /// See [`InvalidSearch`].
    pub fn new(text: &str, limit: usize) -> Result<Self, InvalidSearch> {
        if limit == 0 {
            return Err(InvalidSearch::ZeroLimit);
        }
        if limit > MAX_RESULTS {
            return Err(InvalidSearch::LimitTooLarge);
        }
        if text.len() > MAX_QUERY_BYTES {
            return Err(InvalidSearch::TooLong);
        }
        let words: Vec<&str> = text
            .split(|character: char| character.is_whitespace() || character.is_control())
            .filter(|word| !word.is_empty())
            .collect();
        match words.len() {
            0 => Err(InvalidSearch::Empty),
            count if count > MAX_QUERY_TERMS => Err(InvalidSearch::TooManyTerms),
            _ => Ok(Self {
                expression: words
                    .iter()
                    .map(|word| quoted(word))
                    .collect::<Vec<_>>()
                    .join(" "),
                limit,
            }),
        }
    }

    /// The most hits this search returns.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

/// A word as a literal phrase: wrapped in quotes, with quotes inside doubled.
fn quoted(word: &str) -> String {
    format!("\"{}\"", word.replace('"', "\"\""))
}

/// One matching chunk with what a citation needs.
pub struct SearchHit {
    /// The page the chunk belongs to.
    pub page: PageId,
    /// The page's title.
    pub title: PageTitle,
    /// The revision the chunk was cut from.
    pub revision: RevisionId,
    /// When that revision was saved.
    pub timestamp: RevisionTimestamp,
    /// The chunk, with its section, source range and losses.
    pub chunk: StoredChunk,
}

impl std::fmt::Debug for SearchHit {
    /// Shows identifiers only: title and text are export content.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SearchHit")
            .field("page", &self.page)
            .field("revision", &self.revision)
            .field("chunk", &self.chunk)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// The best chunk matches in `source` for `query`, best first, at most
    /// the query's limit. An unknown source has no matches.
    ///
    /// # Errors
    ///
    /// Fails on a database error or if a stored value is invalid. A query with
    /// no matches is an empty list, not an error.
    pub fn search(
        &self,
        source: &SourceId,
        query: &SearchQuery,
    ) -> Result<Vec<SearchHit>, StoreError> {
        let Some(source_key) = find_source(&self.connection, source)? else {
            return Ok(Vec::new());
        };
        let mut statement = self
            .connection
            .prepare(
                "SELECT chunk.page_id, page.title, page.revision_id, page.revision_time,
                        chunk.section_index, chunk.chunk_index, chunk.section_path,
                        chunk.section_occurrence, chunk.section_level, chunk.source_start,
                        chunk.source_end, chunk.kind, chunk.losses, chunk.templates,
                        chunk.link_targets, chunk.text
                 FROM chunk_fts
                 JOIN chunk ON chunk.id = chunk_fts.rowid
                 JOIN page ON page.source_id = chunk.source_id AND page.page_id = chunk.page_id
                 WHERE chunk_fts MATCH ?1 AND chunk.source_id = ?2
                 ORDER BY chunk_fts.rank, chunk.page_id, chunk.section_index, chunk.chunk_index
                 LIMIT ?3",
            )
            .map_err(StoreError::database(Operation::Read))?;
        let limit = i64::try_from(query.limit).map_err(|_| StoreError::IdentifierTooLarge)?;
        let rows = statement
            .query_map(params![query.expression, source_key, limit], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    chunk_row(row, 4)?,
                ))
            })
            .map_err(StoreError::database(Operation::Read))?;
        rows.map(|row| hit(row.map_err(StoreError::database(Operation::Read))?))
            .collect()
    }
}

type HitRow = (i64, String, i64, String, super::ChunkRow);

fn hit(row: HitRow) -> Result<SearchHit, StoreError> {
    let (page, title, revision, time, chunk) = row;
    let invalid = |what| StoreError::Corrupt { what };
    Ok(SearchHit {
        page: u64::try_from(page)
            .ok()
            .and_then(PageId::new)
            .ok_or_else(|| invalid("page id"))?,
        title: PageTitle::new(title).ok_or_else(|| invalid("title"))?,
        revision: u64::try_from(revision)
            .ok()
            .and_then(RevisionId::new)
            .ok_or_else(|| invalid("revision id"))?,
        timestamp: RevisionTimestamp::parse(&time).map_err(|_| invalid("revision time"))?,
        chunk: stored_chunk(chunk)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fixture, import, page, page_id};
    use super::*;
    use crate::wiki::ImportedPage;

    fn query(text: &str) -> SearchQuery {
        SearchQuery::new(text, 10).unwrap()
    }

    fn titled(id: u64, title: &str, text: &str) -> ImportedPage {
        let mut result = page(id, id * 10, text);
        result.title = PageTitle::new(title.to_owned()).unwrap();
        result
    }

    fn store_with(fixture: &Fixture, pages: &[ImportedPage]) -> Store {
        let mut store = fixture.open();
        import(&mut store, &fixture.source, pages);
        store
    }

    fn found_pages(store: &Store, fixture: &Fixture, text: &str) -> Vec<u64> {
        store
            .search(&fixture.source, &query(text))
            .unwrap()
            .iter()
            .map(|hit| hit.page.get())
            .collect()
    }

    #[test]
    fn each_word_becomes_a_quoted_literal_term() {
        assert_eq!(
            SearchQuery::new("  dragon\taxe \n", 5).unwrap().expression,
            "\"dragon\" \"axe\""
        );
        assert_eq!(
            SearchQuery::new("say \"hi\" OR title:x*", 5)
                .unwrap()
                .expression,
            "\"say\" \"\"\"hi\"\"\" \"OR\" \"title:x*\""
        );
        assert_eq!(
            SearchQuery::new("a\u{0}b", 5).unwrap().expression,
            "\"a\" \"b\""
        );
    }

    #[test]
    fn invalid_queries_are_refused_by_kind_without_echoing_the_text() {
        assert_eq!(SearchQuery::new("", 5), Err(InvalidSearch::Empty));
        assert_eq!(SearchQuery::new(" \t\n\u{0}", 5), Err(InvalidSearch::Empty));
        assert_eq!(SearchQuery::new("a", 0), Err(InvalidSearch::ZeroLimit));
        assert_eq!(
            SearchQuery::new("a", MAX_RESULTS + 1),
            Err(InvalidSearch::LimitTooLarge)
        );
        assert_eq!(
            SearchQuery::new(&"a".repeat(MAX_QUERY_BYTES + 1), 5),
            Err(InvalidSearch::TooLong)
        );
        let many = vec!["a"; MAX_QUERY_TERMS + 1].join(" ");
        assert_eq!(SearchQuery::new(&many, 5), Err(InvalidSearch::TooManyTerms));
    }

    #[test]
    fn the_bounds_are_inclusive() {
        assert!(SearchQuery::new(&"a".repeat(MAX_QUERY_BYTES), MAX_RESULTS).is_ok());
        let exact = vec!["a"; MAX_QUERY_TERMS].join(" ");
        assert!(SearchQuery::new(&exact, 1).is_ok());
    }

    #[test]
    fn a_query_debug_shows_the_limit_but_not_the_words() {
        let shown = format!("{:?}", SearchQuery::new("secret words", 7).unwrap());
        assert!(shown.contains('7'), "{shown}");
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn a_word_in_the_chunk_text_finds_the_chunk_with_its_citation() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                titled(1, "Axe", "Chops trees fast."),
                titled(2, "Pick", "Mines rocks."),
            ],
        );
        let hits = store.search(&fixture.source, &query("trees")).unwrap();
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.page, page_id(1));
        assert_eq!(hit.title.as_str(), "Axe");
        assert_eq!(hit.revision.get(), 10);
        assert_eq!(hit.timestamp.as_str(), "2026-01-02T03:04:05Z");
        assert_eq!(hit.chunk.text, "Chops trees fast.");
    }

    #[test]
    fn titles_and_section_headings_are_searchable() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                titled(1, "Zulrah", "Serpent."),
                titled(2, "Bank", "== Placeholders ==\nSlots."),
            ],
        );
        assert_eq!(found_pages(&store, &fixture, "zulrah"), [1]);
        assert_eq!(found_pages(&store, &fixture, "placeholders"), [2]);
    }

    #[test]
    fn every_word_must_match_in_any_order_ignoring_case_and_accents() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                titled(1, "A", "Crème brûlée recipe"),
                titled(2, "B", "recipe only"),
            ],
        );
        assert_eq!(found_pages(&store, &fixture, "RECIPE creme"), [1]);
        let mut both = found_pages(&store, &fixture, "recipe");
        both.sort_unstable();
        assert_eq!(both, [1, 2]);
        assert_eq!(
            found_pages(&store, &fixture, "recipe absent"),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn query_syntax_is_searched_for_not_interpreted() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                titled(1, "A", "axe or pick"),
                titled(2, "B", "axe"),
                titled(3, "C", "pick"),
            ],
        );
        // As syntax, `OR` would match page 2 or 3 and `NOT` would exclude.
        assert_eq!(found_pages(&store, &fixture, "axe OR pick"), [1]);
        assert_eq!(
            found_pages(&store, &fixture, "axe NOT pick"),
            Vec::<u64>::new()
        );
        // As syntax, a column filter or prefix mark would match; as words they
        // are the text "title axe" and "ax".
        assert_eq!(
            found_pages(&store, &fixture, "title:axe"),
            Vec::<u64>::new()
        );
        assert_eq!(found_pages(&store, &fixture, "ax*"), Vec::<u64>::new());
    }

    #[test]
    fn hostile_input_never_makes_the_index_fail() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "A", "axe")]);
        for text in [
            "\"",
            "\"\"",
            "\"unbalanced",
            "(",
            ")",
            "NEAR(a b)",
            "-axe",
            "^axe",
            "a AND",
            "*",
            "...",
            "'; DROP TABLE chunk; --",
            "{{Template|x}}",
            "[[Link]]",
            "é",
            "\u{1F5E1}",
            "title:",
            "{title body}: axe",
            "OR",
            "NOT",
            "+",
            "a:b:c",
        ] {
            assert!(
                SearchQuery::new(text, 10)
                    .map(|search| store.search(&fixture.source, &search))
                    .is_ok_and(|result| result.is_ok()),
                "{text:?}"
            );
        }
        assert_eq!(found_pages(&store, &fixture, "axe"), [1]);
    }

    #[test]
    fn the_limit_caps_the_hits_and_the_best_ones_are_kept() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                titled(1, "A", "needle"),
                titled(2, "B", "needle needle needle"),
                titled(3, "C", "needle"),
            ],
        );
        let one = store
            .search(&fixture.source, &SearchQuery::new("needle", 1).unwrap())
            .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].page, page_id(2));
    }

    #[test]
    fn equal_matches_are_ordered_by_page_id_and_repeat_exactly() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                titled(9, "A", "needle"),
                titled(2, "B", "needle"),
                titled(5, "C", "needle"),
            ],
        );
        assert_eq!(found_pages(&store, &fixture, "needle"), [2, 5, 9]);
        assert_eq!(found_pages(&store, &fixture, "needle"), [2, 5, 9]);
    }

    #[test]
    fn chunks_of_one_page_come_in_document_order_when_equally_relevant() {
        let fixture = Fixture::new();
        let text = "needle one\n\n== Second ==\nneedle two\n\n== Third ==\nneedle six";
        let store = store_with(&fixture, &[titled(1, "A", text)]);
        let order: Vec<usize> = store
            .search(&fixture.source, &query("needle"))
            .unwrap()
            .iter()
            .map(|hit| hit.chunk.section)
            .collect();
        assert_eq!(order, [0, 1, 2]);
    }

    #[test]
    fn a_replaced_revision_is_searchable_by_its_new_text_and_title_only() {
        let fixture = Fixture::new();
        let mut store = store_with(&fixture, &[titled(1, "Old name", "ancient words")]);
        let mut newer = titled(1, "New name", "fresh words");
        newer.revision = RevisionId::new(11).unwrap();
        import(&mut store, &fixture.source, &[newer]);
        assert_eq!(found_pages(&store, &fixture, "ancient"), Vec::<u64>::new());
        assert_eq!(found_pages(&store, &fixture, "old"), Vec::<u64>::new());
        assert_eq!(found_pages(&store, &fixture, "fresh"), [1]);
        assert_eq!(found_pages(&store, &fixture, "new"), [1]);
    }

    #[test]
    fn a_redirect_has_no_chunks_to_find() {
        let fixture = Fixture::new();
        let mut redirect = titled(1, "Short", "#REDIRECT [[Long]]");
        redirect.redirect_target = PageTitle::new("Long".to_owned());
        let store = store_with(&fixture, &[redirect, titled(2, "Long", "body")]);
        assert_eq!(found_pages(&store, &fixture, "short"), Vec::<u64>::new());
        assert_eq!(found_pages(&store, &fixture, "redirect"), Vec::<u64>::new());
    }

    #[test]
    fn results_stay_within_their_source() {
        let fixture = Fixture::new();
        let other = SourceId::new(
            "Other Wiki".to_owned(),
            None,
            "https://other.example/wiki/Main_Page".to_owned(),
        )
        .unwrap();
        let mut store = store_with(&fixture, &[titled(1, "A", "needle")]);
        import(&mut store, &other, &[titled(1, "A", "needle elsewhere")]);
        let here = store.search(&fixture.source, &query("needle")).unwrap();
        assert_eq!(here.len(), 1);
        assert_eq!(here[0].chunk.text, "needle");
        let unknown = SourceId::new(
            "Third".to_owned(),
            None,
            "https://third.example/".to_owned(),
        )
        .unwrap();
        assert_eq!(store.search(&unknown, &query("needle")).unwrap().len(), 0);
    }

    #[test]
    fn the_index_survives_closing_and_reopening_the_database() {
        let fixture = Fixture::new();
        drop(store_with(&fixture, &[titled(1, "A", "needle")]));
        assert_eq!(found_pages(&fixture.open(), &fixture, "needle"), [1]);
    }

    #[test]
    fn a_failed_import_leaves_the_index_as_it_was() {
        let fixture = Fixture::new();
        let mut store = store_with(&fixture, &[titled(1, "A", "needle")]);
        let mut session = store
            .begin_import(&fixture.source, crate::wiki::PrepareLimits::default())
            .unwrap();
        session.add(&titled(2, "B", "needle two")).unwrap();
        drop(session);
        assert_eq!(found_pages(&store, &fixture, "needle"), [1]);
    }

    #[test]
    fn hit_debug_shows_identifiers_but_no_title_or_text() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Secret title", "secret body")]);
        let hits = store.search(&fixture.source, &query("secret")).unwrap();
        let shown = format!("{hits:?}");
        assert!(
            !shown.contains("Secret") && !shown.contains("secret"),
            "{shown}"
        );
    }
}
