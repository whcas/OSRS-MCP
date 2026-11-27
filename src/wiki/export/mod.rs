//! Streaming importer for manually exported `MediaWiki` XML.
//!
//! [`ExportReader`] reads a `Special:Export` file from any [`BufRead`] and
//! yields one [`ImportedPage`] per page, in file order, without holding the
//! whole export in memory. It is offline and pure: it opens no files, makes no
//! requests and never interprets wikitext.
//!
//! # Contract
//!
//! - **Schema:** `MediaWiki` export schema **0.11**, which is the namespace
//!   `http://www.mediawiki.org/xml/export-0.11/` and `version="0.11"` on the
//!   root element. This is the version in the one real sample the maintainer
//!   examined (generator `MediaWiki` 1.45.3); other versions are refused with
//!   [`ExportError::UnsupportedSchema`], not guessed at. The sample is not
//!   committed, so the schema is otherwise known from the published format.
//! - **Current revisions only:** a page with several revisions (a history
//!   export) is skipped with [`PageIssueKind::MultipleRevisions`]. Only
//!   `wikitext` content in format `text/x-wiki` is imported; other content
//!   models are skipped with [`PageIssueKind::UnsupportedContent`]. A missing
//!   model or format is taken to be wikitext.
//! - **Kept:** site identity from `siteinfo`, and per page its id, namespace,
//!   title, revision id, timestamp, raw wikitext, the reported hash and any
//!   redirect target. The hash is not verified (see ADR-0003).
//! - **Dropped by design:** contributor names and ids, edit comments, minor
//!   flags, parent revision ids and all other elements. Nothing downstream
//!   needs them, and attribution is by title and revision.
//! - **Redirects:** `<redirect title="…"/>` is recorded as
//!   [`ImportedPage::redirect_target`]. The target page is never fabricated and
//!   need not be in the export.
//! - **Text:** exactly as the XML decodes: entity and character references
//!   resolved, CDATA taken literally, line endings normalised to `\n`.
//! - **Duplicates:** if a page id appears again, the first page is kept and the
//!   later one is reported as [`PageIssueKind::DuplicatePage`].
//! - **Content after the root's closing tag is ignored.**
//!
//! # Errors versus issues
//!
//! A problem confined to one page is a [`PageIssue`]: that page is skipped and
//! reading continues, so one bad page does not hide the rest. A problem that
//! makes the rest of the stream unreliable (malformed or truncated XML, an
//! exceeded input, page or depth limit, an unreadable source) is an
//! [`ExportError`]: it is the last item the iterator yields. A bad header is
//! reported by [`ExportReader::open`]. [`ExportReader::summary`] counts both.
//!
//! # Safety limits
//!
//! [`ImportLimits`] bounds the total input, one page's XML, one revision's text
//! and the nesting depth, with defaults far above any real export. Document
//! type declarations are refused outright, so no entity is ever defined or
//! expanded, and nothing external is loaded. Errors contain byte offsets, page
//! ordinals and numeric ids only, never text from the file.
//!
//! Page and input limits are enforced as bytes are read, so an oversized
//! element is never buffered in full. They end the import, because the rest of
//! an element that was cut short cannot be skipped reliably. The text limit
//! applies to a page already read in full, so an over-limit revision skips only
//! that page.

mod error;
mod header;
mod input;
mod page;
mod reader;
mod tokens;

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};

pub use error::{
    ExportError, ImportIssue, LimitKind, PageField, PageIssue, PageIssueKind, XmlFault,
};

pub use reader::ExportReader;

use super::ImportedPage;

const DEFAULT_MAX_INPUT_BYTES: NonZeroU64 = non_zero_u64(1 << 30);
const DEFAULT_MAX_PAGE_BYTES: NonZeroU64 = non_zero_u64(16 << 20);
const DEFAULT_MAX_TEXT_BYTES: NonZeroUsize = non_zero_usize(4 << 20);
const DEFAULT_MAX_DEPTH: NonZeroUsize = non_zero_usize(32);

const fn non_zero_u64(value: u64) -> NonZeroU64 {
    match NonZeroU64::new(value) {
        Some(value) => value,
        None => NonZeroU64::MIN,
    }
}

const fn non_zero_usize(value: usize) -> NonZeroUsize {
    match NonZeroUsize::new(value) {
        Some(value) => value,
        None => NonZeroUsize::MIN,
    }
}

/// Resource limits for one import. Every limit is at least one.
///
/// The defaults are generous: the default text limit is twice `MediaWiki`'s
/// own maximum page size, and a typical manual export is far below the input
/// limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportLimits {
    /// Most bytes read from the input in total. Default 1 GiB.
    pub max_input_bytes: NonZeroU64,
    /// Most bytes of XML in one `<page>` element, escaping included, counted
    /// from after its opening tag to the end of its closing tag. Default
    /// 16 MiB.
    pub max_page_bytes: NonZeroU64,
    /// Most bytes of wikitext in one revision, after unescaping. Default 4 MiB.
    pub max_text_bytes: NonZeroUsize,
    /// Deepest element nesting, counting the root as 1. Default 32. Real
    /// exports nest at most five levels.
    pub max_depth: NonZeroUsize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_page_bytes: DEFAULT_MAX_PAGE_BYTES,
            max_text_bytes: DEFAULT_MAX_TEXT_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

/// What an import has produced so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// Pages yielded, redirects included.
    pub pages_imported: u64,
    /// How many of the imported pages are redirects.
    pub redirects_imported: u64,
    /// Skipped pages, counted by reason.
    pub pages_skipped: BTreeMap<PageIssueKind, u64>,
    /// The error that ended the import early, if one did.
    pub stream_error: Option<ExportError>,
}

impl ExportSummary {
    /// The number of pages skipped for any reason.
    #[must_use]
    pub fn total_pages_skipped(&self) -> u64 {
        self.pages_skipped.values().sum()
    }

    pub(super) const fn record_import(&mut self, page: &ImportedPage) {
        self.pages_imported += 1;
        if page.redirect_target.is_some() {
            self.redirects_imported += 1;
        }
    }

    pub(super) fn record_skip(&mut self, issue: &PageIssue) {
        *self.pages_skipped.entry(issue.kind).or_default() += 1;
    }
}
