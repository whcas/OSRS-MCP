//! Section-aware preparation of imported wikitext for retrieval.
//!
//! [`prepare`] splits one [`ImportedPage`] into its sections and into bounded
//! chunks, each carrying the provenance a citation needs: page, revision,
//! section path and the exact byte range of the raw wikitext it came from. It is
//! a pure function: the same page and limits always give the same result, and
//! it reads nothing but the page.
//!
//! # What it does and does not do
//!
//! - Headings are `== Title ==` lines at the start of a line, levels 1 to 6.
//!   Lines inside comments, `nowiki`, `pre`, `syntaxhighlight`, `source` and
//!   `math` tags, and multi-line template calls are not headings.
//! - Text before the first heading is the **lead** section (empty path).
//! - A section's path is the titles of its enclosing headings. Duplicate paths
//!   are told apart by [`SectionId::occurrence`]; ids are stable for one
//!   revision, not across revisions.
//! - Chunk text is a verbatim, whitespace-trimmed slice of the raw wikitext.
//!   Nothing is expanded, rendered, summarised or rewritten, so no text or
//!   number can appear that the page did not contain. Markup, templates and
//!   links stay raw. The raw text itself stays on the [`ImportedPage`].
//! - Heading titles over 512 bytes are an error ([`PrepareError::HeadingTooLong`]),
//!   and a chunk refers to its section by position, so the size of a prepared
//!   page is bounded by the text plus at most 2000 sections of at most six
//!   short titles each, not by chunks times path length.
//! - A section is never mixed with its neighbours: a chunk belongs to exactly
//!   one section. Chunks prefer paragraph boundaries, then boundaries between
//!   blocks: lines, list items, table rows and multi-line template calls are
//!   kept whole when they fit, and a table that fits is kept whole too. They cut
//!   between the lines of one block only when it exceeds the bound, and inside
//!   a line only when one line does.
//! - A caveat (a line opening with `Note:`, `Notes:`, `Note that`, `However`
//!   or a `<ref` footnote) is kept in the chunk of the claim before it when the
//!   two fit together. Where a boundary still falls inside a table, markup,
//!   line or between a claim and its caveat, both neighbouring chunks list the
//!   matching [`Loss`].
//! - A redirect page has no sections or chunks; its target is carried on the
//!   [`PreparedPage`].
//!
//! All page text is untrusted data. Preparation executes and requests nothing,
//! and errors carry counts and limits only.

mod chunk;
mod scan;
mod structure;

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::ops::Range;

use thiserror::Error;

use super::{ImportedPage, PageId, PageTitle, RevisionId};
use scan::{Heading, ScanError};

const DEFAULT_MAX_CHUNK_BYTES: usize = 2000;
const DEFAULT_MAX_SECTIONS: usize = 2000;
/// Every UTF-8 character fits in four bytes, so a smaller bound could not make
/// progress through some text.
const MIN_CHUNK_BYTES: usize = 4;

/// The limits for one preparation.
///
/// Create with [`PrepareLimits::new`] or take the [`Default`] (chunks of at
/// most 2000 bytes, at most 2000 sections including the lead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareLimits {
    max_chunk_bytes: NonZeroUsize,
    max_sections: NonZeroUsize,
}

/// The limits given to [`PrepareLimits::new`] are not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InvalidLimits {
    /// A chunk must be able to hold any single character.
    #[error("chunk size must be at least {MIN_CHUNK_BYTES} bytes")]
    ChunkTooSmall,
    /// There must be room for at least the lead section.
    #[error("section limit must be at least one")]
    NoSections,
}

impl PrepareLimits {
    /// Builds limits.
    ///
    /// `max_chunk_bytes` bounds each chunk's text, in bytes. `max_sections`
    /// bounds the sections of a page, counting the lead.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidLimits`] if `max_chunk_bytes` is below 4 or
    /// `max_sections` is zero.
    pub const fn new(max_chunk_bytes: usize, max_sections: usize) -> Result<Self, InvalidLimits> {
        if max_chunk_bytes < MIN_CHUNK_BYTES {
            return Err(InvalidLimits::ChunkTooSmall);
        }
        match (
            NonZeroUsize::new(max_chunk_bytes),
            NonZeroUsize::new(max_sections),
        ) {
            (Some(max_chunk_bytes), Some(max_sections)) => Ok(Self {
                max_chunk_bytes,
                max_sections,
            }),
            (None, _) => Err(InvalidLimits::ChunkTooSmall),
            (_, None) => Err(InvalidLimits::NoSections),
        }
    }

    /// The largest chunk, in bytes.
    #[must_use]
    pub const fn max_chunk_bytes(&self) -> usize {
        self.max_chunk_bytes.get()
    }

    /// The most sections a page may have, counting the lead.
    #[must_use]
    pub const fn max_sections(&self) -> usize {
        self.max_sections.get()
    }
}

impl Default for PrepareLimits {
    fn default() -> Self {
        Self {
            max_chunk_bytes: NonZeroUsize::new(DEFAULT_MAX_CHUNK_BYTES)
                .unwrap_or(NonZeroUsize::MIN),
            max_sections: NonZeroUsize::new(DEFAULT_MAX_SECTIONS).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// Why a page could not be prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PrepareError {
    /// The page has more sections than the limit allows.
    #[error("page has more than {max} sections")]
    TooManySections {
        /// The configured limit.
        max: usize,
    },
    /// A heading title is longer than the fixed limit, which keeps section
    /// paths small.
    #[error("a section heading is longer than {max} bytes")]
    HeadingTooLong {
        /// The limit, in bytes.
        max: usize,
    },
}

/// The titles of a section's enclosing headings, outermost first. The lead
/// section's path is empty.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SectionPath(Vec<String>);

impl fmt::Debug for SectionPath {
    /// Shows the depth only: titles are untrusted export text, and `Debug`
    /// output ends up in logs and failed-assertion messages. Use `Display` to
    /// print a path deliberately.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SectionPath(depth {})", self.0.len())
    }
}

impl SectionPath {
    /// The heading titles, outermost first, as written (markup is not removed).
    #[must_use]
    pub fn titles(&self) -> &[String] {
        &self.0
    }

    /// Whether this is the lead section's path.
    #[must_use]
    pub const fn is_lead(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for SectionPath {
    /// Titles joined with ` > `, or `(lead)` for the lead section.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("(lead)");
        }
        formatter.write_str(&self.0.join(" > "))
    }
}

/// Identifies a section within one revision of a page.
///
/// Two sections with the same heading path (a repeated heading) differ by
/// `occurrence`, which counts earlier sections with that path from zero.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionId {
    /// The enclosing headings.
    pub path: SectionPath,
    /// How many earlier sections of this revision have the same path.
    pub occurrence: u32,
}

/// One section of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The section's identifier.
    pub id: SectionId,
    /// The heading level, 1 to 6, or 0 for the lead.
    pub level: u8,
    /// The byte range of the section's body in the page's wikitext: after its
    /// heading line, up to the next heading of any level.
    pub body: Range<usize>,
}

/// What a chunk boundary or the chunk's content leaves out, so that a reader
/// knows the text may mislead on its own. Only the boundaries are listed:
/// chunks are verbatim, so nothing inside them is altered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Loss {
    /// An edge of the chunk falls inside a line, because that line was longer
    /// than the chunk bound. Markup on that line may be cut in two.
    CutLine,
    /// An edge falls between the lines of one multi-line template call,
    /// comment or literal tag, which was too long to keep whole.
    SplitMarkup,
    /// An edge falls between two parts of one table, so rows may lack the
    /// table's header or the rest of the table.
    SplitTable,
    /// An edge separates a caveat (see the module documentation) from the
    /// claim before it, because the two did not fit in one chunk.
    SplitCaveat,
}

/// A bounded retrieval unit: verbatim wikitext from one section.
///
/// A chunk refers to its section by position instead of owning a copy of its
/// path, so the size of a prepared page stays proportional to the text. Its
/// `Debug` output omits the text.
#[derive(Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position of the section the text comes from in
    /// [`PreparedPage::sections`]. Use [`PreparedPage::section_of`].
    pub section: usize,
    /// The position of this chunk among its section's chunks, from zero.
    pub index: usize,
    /// The byte range of the text within the page's wikitext.
    pub source: Range<usize>,
    /// The wikitext at `source`, trimmed of surrounding whitespace.
    pub text: String,
    /// What the chunk's edges damage, without repeats, in [`Loss`] order.
    pub losses: Vec<Loss>,
}

impl Chunk {
    /// Whether the chunk's edges cause `loss`.
    #[must_use]
    pub fn has_loss(&self, loss: Loss) -> bool {
        self.losses.contains(&loss)
    }
}

impl fmt::Debug for Chunk {
    /// Omits the wikitext, like [`ImportedPage`](super::ImportedPage).
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Chunk")
            .field("section", &self.section)
            .field("index", &self.index)
            .field("source", &self.source)
            .field("text_bytes", &self.text.len())
            .field("losses", &self.losses)
            .finish()
    }
}

/// A page split into sections and chunks, with the provenance to cite them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPage {
    /// The page the text came from.
    pub page: PageId,
    /// The revision the text came from.
    pub revision: RevisionId,
    /// The page title.
    pub title: PageTitle,
    /// The declared redirect target. A redirect has no sections or chunks.
    pub redirect_target: Option<PageTitle>,
    /// Sections in document order, lead first. Empty for a redirect.
    pub sections: Vec<Section>,
    /// Chunks in document order. Empty for a redirect.
    pub chunks: Vec<Chunk>,
}

impl PreparedPage {
    /// The section a chunk of this page belongs to, or `None` if the chunk is
    /// from another page.
    #[must_use]
    pub fn section_of(&self, chunk: &Chunk) -> Option<&Section> {
        self.sections.get(chunk.section)
    }
}

/// Splits a page into sections and bounded chunks.
///
/// See the [module documentation](self) for what is and is not done to the
/// text.
///
/// # Errors
///
/// Returns [`PrepareError::TooManySections`] if the page has more headings
/// than `limits` allow.
pub fn prepare(page: &ImportedPage, limits: &PrepareLimits) -> Result<PreparedPage, PrepareError> {
    let (sections, chunks) = if page.redirect_target.is_some() {
        (Vec::new(), Vec::new())
    } else {
        let sections = split_sections(&page.text, limits)?;
        let chunks = chunk_sections(&page.text, &sections, limits);
        (sections, chunks)
    };
    Ok(PreparedPage {
        page: page.id,
        revision: page.revision,
        title: page.title.clone(),
        redirect_target: page.redirect_target.clone(),
        sections,
        chunks,
    })
}

/// Sections in document order, with paths and ids assigned. The lead is
/// included even when empty, so every page has at least one section.
fn split_sections(text: &str, limits: &PrepareLimits) -> Result<Vec<Section>, PrepareError> {
    // The lead takes one of the allowed sections.
    let headings =
        scan::find_headings(text, limits.max_sections().saturating_sub(1)).map_err(|error| {
            match error {
                ScanError::TooManyHeadings => PrepareError::TooManySections {
                    max: limits.max_sections(),
                },
                ScanError::HeadingTooLong => PrepareError::HeadingTooLong {
                    max: scan::MAX_HEADING_BYTES,
                },
            }
        })?;
    let mut builder = SectionBuilder::default();
    let lead_end = headings
        .first()
        .map_or(text.len(), |first| first.line.start);
    builder.push_lead(0..lead_end);
    for (index, heading) in headings.iter().enumerate() {
        let body_end = headings
            .get(index + 1)
            .map_or(text.len(), |next| next.line.start);
        builder.push_heading(heading, heading.body_start.min(body_end)..body_end);
    }
    Ok(builder.sections)
}

/// Tracks the heading stack and duplicate paths while sections are added.
#[derive(Default)]
struct SectionBuilder {
    sections: Vec<Section>,
    /// How many sections so far have each path, for duplicate headings.
    path_counts: HashMap<SectionPath, u32>,
    /// Enclosing headings: their levels and titles.
    stack: Vec<(u8, String)>,
}

impl SectionBuilder {
    fn push_lead(&mut self, body: Range<usize>) {
        self.push(SectionPath::default(), 0, body);
    }

    fn push_heading(&mut self, heading: &Heading, body: Range<usize>) {
        while self
            .stack
            .last()
            .is_some_and(|(level, _)| *level >= heading.level)
        {
            self.stack.pop();
        }
        self.stack.push((heading.level, heading.title.clone()));
        let path = SectionPath(self.stack.iter().map(|(_, title)| title.clone()).collect());
        self.push(path, heading.level, body);
    }

    fn push(&mut self, path: SectionPath, level: u8, body: Range<usize>) {
        let seen = self.path_counts.entry(path.clone()).or_insert(0);
        let occurrence = *seen;
        *seen = seen.saturating_add(1);
        self.sections.push(Section {
            id: SectionId { path, occurrence },
            level,
            body,
        });
    }
}

fn chunk_sections(text: &str, sections: &[Section], limits: &PrepareLimits) -> Vec<Chunk> {
    sections
        .iter()
        .enumerate()
        .flat_map(|(section_index, section)| {
            let body = &text[section.body.clone()];
            chunk::split(body, limits.max_chunk_bytes())
                .into_iter()
                .enumerate()
                .map(move |(index, piece)| {
                    let source = section.body.start + piece.range.start
                        ..section.body.start + piece.range.end;
                    Chunk {
                        section: section_index,
                        index,
                        text: text[source.clone()].to_owned(),
                        source,
                        losses: piece.losses,
                    }
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::RevisionTimestamp;

    fn page(text: &str) -> ImportedPage {
        ImportedPage {
            id: PageId::new(7).unwrap(),
            namespace: 0,
            title: PageTitle::new("Example".to_owned()).unwrap(),
            revision: RevisionId::new(70).unwrap(),
            timestamp: RevisionTimestamp::parse("2026-01-02T03:04:05Z").unwrap(),
            hash: None,
            redirect_target: None,
            text: text.to_owned(),
        }
    }

    fn limits(chunk: usize, sections: usize) -> PrepareLimits {
        PrepareLimits::new(chunk, sections).unwrap()
    }

    fn prepared(text: &str) -> PreparedPage {
        prepare(&page(text), &PrepareLimits::default()).unwrap()
    }

    fn paths(prepared: &PreparedPage) -> Vec<String> {
        prepared
            .sections
            .iter()
            .map(|section| section.id.path.to_string())
            .collect()
    }

    #[test]
    fn a_page_without_headings_is_one_lead_section() {
        let result = prepared("Just some text.\n\nMore text.");
        assert_eq!(paths(&result), ["(lead)"]);
        assert_eq!(result.sections[0].level, 0);
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].text, "Just some text.\n\nMore text.");
    }

    #[test]
    fn the_lead_is_always_first_even_when_empty() {
        let result = prepared("== First ==\nbody");
        assert_eq!(paths(&result), ["(lead)", "First"]);
        assert!(result.sections[0].body.is_empty());
        assert_eq!(result.chunks.len(), 1);
        let section = result.section_of(&result.chunks[0]).unwrap();
        assert_eq!(section.id.path.to_string(), "First");
    }

    #[test]
    fn an_empty_page_has_only_an_empty_lead_and_no_chunks() {
        let result = prepared("");
        assert_eq!(paths(&result), ["(lead)"]);
        assert_eq!(result.chunks.len(), 0);
    }

    #[test]
    fn paths_follow_the_heading_hierarchy() {
        let text =
            "intro\n== A ==\na\n=== A1 ===\na1\n=== A2 ===\na2\n== B ==\nb\n=== B1 ===\nb1\n";
        assert_eq!(
            paths(&prepared(text)),
            ["(lead)", "A", "A > A1", "A > A2", "B", "B > B1"]
        );
    }

    #[test]
    fn a_skipped_level_nests_under_the_nearest_shallower_heading() {
        let text = "== A ==\na\n==== Deep ====\nd\n== B ==\n==== Deep ====\nd2\n";
        assert_eq!(
            paths(&prepared(text)),
            ["(lead)", "A", "A > Deep", "B", "B > Deep"]
        );
    }

    #[test]
    fn a_shallower_heading_after_a_deeper_one_closes_the_deeper_sections() {
        let text = "=== Low ===\nx\n= Top =\ny\n== Mid ==\nz\n";
        assert_eq!(
            paths(&prepared(text)),
            ["(lead)", "Low", "Top", "Top > Mid"]
        );
    }

    #[test]
    fn repeated_headings_get_distinct_stable_ids() {
        let text = "== Notes ==\none\n== Other ==\n== Notes ==\ntwo\n== Notes ==\nthree\n";
        let result = prepared(text);
        let notes: Vec<u32> = result
            .sections
            .iter()
            .filter(|section| section.id.path.to_string() == "Notes")
            .map(|section| section.id.occurrence)
            .collect();
        assert_eq!(notes, [0, 1, 2]);
        let ids: std::collections::HashSet<_> =
            result.sections.iter().map(|section| &section.id).collect();
        assert_eq!(ids.len(), result.sections.len());
    }

    #[test]
    fn the_same_heading_under_different_parents_has_different_paths() {
        let text = "== A ==\n=== Notes ===\n== B ==\n=== Notes ===\n";
        let result = prepared(text);
        assert!(
            result
                .sections
                .iter()
                .all(|section| section.id.occurrence == 0)
        );
    }

    #[test]
    fn a_section_body_runs_to_the_next_heading_of_any_level() {
        let text = "== A ==\nalpha\n=== A1 ===\nbeta\n== B ==\ngamma";
        let result = prepared(text);
        let bodies: Vec<&str> = result
            .sections
            .iter()
            .map(|section| text[section.body.clone()].trim())
            .collect();
        assert_eq!(bodies, ["", "alpha", "beta", "gamma"]);
    }

    #[test]
    fn chunks_are_verbatim_slices_that_the_source_range_locates() {
        let text =
            "lead\n== A ==\n  '''bold''' {{Tmpl|x=1}} [[Link|label]]  \n\n<!-- note -->\nend";
        let result = prepared(text);
        for chunk in &result.chunks {
            assert_eq!(text[chunk.source.clone()], chunk.text);
            assert_eq!(chunk.text, chunk.text.trim());
        }
        assert_eq!(
            result.chunks[1].text,
            "'''bold''' {{Tmpl|x=1}} [[Link|label]]  \n\n<!-- note -->\nend"
        );
    }

    #[test]
    fn a_chunk_never_spans_two_sections() {
        let text = "tiny\n== A ==\ntiny\n== B ==\ntiny";
        let result = prepared(text);
        let sections: Vec<String> = result
            .chunks
            .iter()
            .map(|chunk| result.sections[chunk.section].id.path.to_string())
            .collect();
        assert_eq!(sections, ["(lead)", "A", "B"]);
    }

    #[test]
    fn long_sections_are_split_and_numbered_in_order() {
        let body = (1..=6)
            .map(|n| format!("paragraph {n}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let text = format!("== Long ==\n{body}");
        let result = prepare(&page(&text), &limits(30, 10)).unwrap();
        let indexes: Vec<usize> = result.chunks.iter().map(|chunk| chunk.index).collect();
        assert_eq!(indexes, [0, 1, 2]);
        assert!(result.chunks.iter().all(|chunk| chunk.text.len() <= 30));
        assert!(result.chunks.iter().all(|chunk| chunk.losses.is_empty()));
    }

    #[test]
    fn indexes_restart_in_each_section() {
        let result = prepare(&page("aa\n\nbb\n== S ==\ncc\n\ndd"), &limits(4, 10)).unwrap();
        let indexes: Vec<(String, usize)> = result
            .chunks
            .iter()
            .map(|chunk| {
                (
                    result.sections[chunk.section].id.path.to_string(),
                    chunk.index,
                )
            })
            .collect();
        assert_eq!(
            indexes,
            [
                ("(lead)".to_owned(), 0),
                ("(lead)".to_owned(), 1),
                ("S".to_owned(), 0),
                ("S".to_owned(), 1)
            ]
        );
    }

    #[test]
    fn a_redirect_has_no_sections_or_chunks_and_keeps_its_target() {
        let mut redirect = page("#REDIRECT [[Target]]");
        redirect.redirect_target = PageTitle::new("Target".to_owned());
        let result = prepare(&redirect, &PrepareLimits::default()).unwrap();
        assert_eq!(result.sections.len(), 0);
        assert_eq!(result.chunks.len(), 0);
        assert_eq!(result.redirect_target, PageTitle::new("Target".to_owned()));
    }

    #[test]
    fn provenance_identifies_the_page_and_revision() {
        let result = prepared("text");
        assert_eq!(result.page.get(), 7);
        assert_eq!(result.revision.get(), 70);
        assert_eq!(result.title.as_str(), "Example");
    }

    #[test]
    fn headings_inside_comments_literals_and_templates_do_not_make_sections() {
        let text = "<!--\n== A ==\n-->\n<nowiki>\n== B ==\n</nowiki>\n{{T\n== C ==\n}}\n== D ==\nx";
        assert_eq!(paths(&prepared(text)), ["(lead)", "D"]);
    }

    #[test]
    fn the_section_limit_counts_the_lead_and_is_exact() {
        let text = "== A ==\n== B ==\n";
        assert_eq!(
            prepare(&page(text), &limits(100, 3)).map(|p| p.sections.len()),
            Ok(3)
        );
        assert_eq!(
            prepare(&page(text), &limits(100, 2)),
            Err(PrepareError::TooManySections { max: 2 })
        );
    }

    #[test]
    fn limits_reject_unusable_values() {
        assert_eq!(PrepareLimits::new(3, 10), Err(InvalidLimits::ChunkTooSmall));
        assert_eq!(PrepareLimits::new(0, 10), Err(InvalidLimits::ChunkTooSmall));
        assert_eq!(PrepareLimits::new(4, 0), Err(InvalidLimits::NoSections));
        assert!(PrepareLimits::new(4, 1).is_ok());
        let default = PrepareLimits::default();
        assert_eq!(
            (default.max_chunk_bytes(), default.max_sections()),
            (2000, 2000)
        );
    }

    #[test]
    fn a_page_with_a_huge_heading_and_many_chunks_cannot_multiply_the_path() {
        // The heading is refused, so no path can be larger than the bound,
        // and chunks hold an index, not a copy of the path.
        let long_title = "t".repeat(600);
        let text = format!("== {long_title} ==\n{}", "word ".repeat(2000));
        assert_eq!(
            prepare(&page(&text), &limits(8, 10)),
            Err(PrepareError::HeadingTooLong { max: 512 })
        );

        let title = "t".repeat(512);
        let text = format!("== {title} ==\n{}", "word ".repeat(2000));
        let result = prepare(&page(&text), &limits(8, 10)).unwrap();
        assert!(result.chunks.len() > 1000);
        assert!(result.chunks.iter().all(|chunk| chunk.section == 1));
        assert_eq!(result.section_of(&result.chunks[0]).unwrap().level, 2);
    }

    #[test]
    fn debug_output_omits_wikitext_titles_and_headings() {
        let text = "lead SECRETMARKER\n== Heading SECRETMARKER ==\nbody SECRETMARKER";
        let mut source = page(text);
        source.redirect_target = None;
        let result = prepare(&source, &PrepareLimits::default()).unwrap();
        let debug = format!("{result:?}");
        assert!(!debug.contains("SECRETMARKER"), "{debug}");
        assert!(!format!("{:?}", result.chunks[0]).contains("SECRETMARKER"));
        assert!(!format!("{:?}", result.sections[1].id).contains("SECRETMARKER"));
    }

    #[test]
    fn preparation_is_deterministic() {
        let text = "lead\n== A ==\none\n\ntwo\n== A ==\nthree\n=== B ===\nfour four four";
        let first = prepare(&page(text), &limits(12, 50)).unwrap();
        let second = prepare(&page(text), &limits(12, 50)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn hostile_text_neither_panics_nor_leaks_into_errors() {
        let marker = "SECRETMARKER";
        let heading_flood = format!("= {marker} =\n").repeat(10);
        let error = prepare(&page(&heading_flood), &limits(100, 5)).unwrap_err();
        assert!(!error.to_string().contains(marker));
        assert!(!format!("{error:?}").contains(marker));
        for text in [
            "{{{{{{{{",
            "}}}}}}",
            "<!--",
            "<nowiki",
            "====",
            "\u{0}\n== \u{0} ==\n",
            "é<",
        ] {
            assert!(
                prepare(&page(text), &PrepareLimits::default()).is_ok(),
                "{text:?}"
            );
        }
    }
}
