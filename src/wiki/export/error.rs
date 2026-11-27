//! Errors and issues reported by the export importer.
//!
//! Nothing here carries text from the export: messages are fixed, and
//! locations are byte offsets, page ordinals and numeric identifiers. A hostile
//! or oversized file therefore cannot produce a hostile or oversized
//! diagnostic.

use std::fmt;
use std::io;

use thiserror::Error;

use crate::wiki::{PageId, SourceField};

/// A resource limit the importer enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LimitKind {
    /// Total bytes read from the input.
    Input,
    /// Bytes of XML in one `<page>` element.
    Page,
    /// Bytes of wikitext in one revision.
    Text,
    /// Nesting depth of XML elements.
    Depth,
}

impl fmt::Display for LimitKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Input => "input size",
            Self::Page => "page size",
            Self::Text => "text size",
            Self::Depth => "nesting depth",
        })
    }
}

/// What was wrong with a document that is not well-formed or not shaped like an
/// export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XmlFault {
    /// The markup does not follow the XML grammar.
    Syntax,
    /// An end tag does not match its start tag, or has none.
    TagMismatch,
    /// A character or entity reference is invalid or not predefined.
    Reference,
    /// An attribute is malformed.
    Attribute,
    /// The bytes are not valid UTF-8.
    Encoding,
    /// A namespace prefix or declaration is invalid.
    Namespace,
    /// The document is well-formed XML but not in the shape of an export.
    Structure,
}

impl fmt::Display for XmlFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Syntax => "invalid XML syntax",
            Self::TagMismatch => "mismatched or missing end tag",
            Self::Reference => "invalid character or entity reference",
            Self::Attribute => "malformed attribute",
            Self::Encoding => "invalid UTF-8",
            Self::Namespace => "invalid namespace",
            Self::Structure => "unexpected structure",
        })
    }
}

/// A failure that ends the import. Pages after it cannot be read reliably.
///
/// Offsets are bytes from the start of the input, as consumed by the parser
/// when the problem was found.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExportError {
    /// The input could not be read. Only the kind is kept; the underlying
    /// message may name a path.
    #[error("reading the export failed: {kind}")]
    Read {
        /// The class of I/O failure.
        kind: io::ErrorKind,
    },
    /// The document is not a `MediaWiki` export at all.
    #[error("input is not a MediaWiki export")]
    NotAnExport,
    /// The export schema is not the supported one (namespace and version 0.11).
    #[error("unsupported export schema; only version 0.11 is supported")]
    UnsupportedSchema,
    /// The XML declaration names an encoding other than UTF-8.
    #[error("unsupported text encoding; only UTF-8 is supported")]
    UnsupportedEncoding,
    /// A document type declaration was present. It is refused rather than
    /// processed, because it can define entities.
    #[error("document type declarations are not allowed (offset {offset})")]
    DoctypeNotAllowed {
        /// Where the declaration was read.
        offset: u64,
    },
    /// The export has no `siteinfo` header before its first page.
    #[error("export has no siteinfo header")]
    MissingSiteInfo,
    /// The `siteinfo` header lacks or repeats a required field, or the field
    /// is invalid.
    #[error("siteinfo {field} is missing, repeated or invalid")]
    InvalidSiteInfo {
        /// The offending field.
        field: SourceField,
    },
    /// The input ended before the document was complete.
    #[error("export is truncated (input ended at offset {offset})")]
    Truncated {
        /// Where the input ended.
        offset: u64,
    },
    /// The document is malformed.
    #[error("export is malformed: {fault} (offset {offset})")]
    Malformed {
        /// What was wrong.
        fault: XmlFault,
        /// Where it was found.
        offset: u64,
    },
    /// A configured limit was exceeded.
    #[error("export exceeds the {limit} limit of {max}")]
    LimitExceeded {
        /// Which limit.
        limit: LimitKind,
        /// The configured maximum, in bytes (or levels for nesting depth).
        max: u64,
    },
}

/// An element of a page the importer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PageField {
    /// The page title.
    Title,
    /// The namespace number.
    Namespace,
    /// The page identifier.
    PageId,
    /// The revision identifier.
    RevisionId,
    /// The revision timestamp.
    Timestamp,
    /// The revision's wikitext.
    Text,
    /// The revision's content hash.
    Hash,
    /// The revision's content model (for example wikitext).
    Model,
    /// The revision's content format (for example `text/x-wiki`).
    Format,
    /// The redirect declaration.
    Redirect,
}

impl fmt::Display for PageField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Title => "title",
            Self::Namespace => "namespace",
            Self::PageId => "page id",
            Self::RevisionId => "revision id",
            Self::Timestamp => "timestamp",
            Self::Text => "text",
            Self::Hash => "hash",
            Self::Model => "content model",
            Self::Format => "content format",
            Self::Redirect => "redirect",
        })
    }
}

/// Why one page was not imported. The rest of the export is unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Error)]
pub enum PageIssueKind {
    /// A required element is absent (or, for text, marked deleted).
    #[error("missing {0}")]
    MissingField(PageField),
    /// An element is present but its value is invalid.
    #[error("invalid {0}")]
    InvalidField(PageField),
    /// An element appears more than once.
    #[error("repeated {0}")]
    DuplicateField(PageField),
    /// The page has no revision.
    #[error("page has no revision")]
    MissingRevision,
    /// The page has more than one revision. Only a current-revision export is
    /// supported, not page history.
    #[error("page has several revisions; only current-revision exports are supported")]
    MultipleRevisions,
    /// The revision is not wikitext (for example a script or JSON page).
    #[error("revision content is not wikitext")]
    UnsupportedContent,
    /// The wikitext is empty or only whitespace.
    #[error("revision text is empty")]
    EmptyText,
    /// The wikitext is larger than the configured text limit.
    #[error("revision text exceeds the text size limit")]
    TextOverLimit,
    /// An earlier page in this export has the same page id; the first was kept.
    #[error("page id already appeared earlier in the export")]
    DuplicatePage,
}

/// A page that was skipped, located by its position in the export.
///
/// The title is deliberately not included: it is export text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("page {ordinal} skipped: {kind}")]
pub struct PageIssue {
    /// One-based position of the `<page>` element in the export.
    pub ordinal: u64,
    /// The page identifier, if it was present and valid.
    pub page_id: Option<PageId>,
    /// What was wrong.
    pub kind: PageIssueKind,
}

/// One problem reported while reading an export.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImportIssue {
    /// A page was skipped; reading continues with the next page.
    #[error(transparent)]
    Page(#[from] PageIssue),
    /// The import cannot continue. This is always the last item.
    #[error(transparent)]
    Stream(#[from] ExportError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_issue_messages_name_the_position_and_not_the_content() {
        let issue = PageIssue {
            ordinal: 3,
            page_id: PageId::new(42),
            kind: PageIssueKind::MissingField(PageField::Timestamp),
        };
        assert_eq!(issue.to_string(), "page 3 skipped: missing timestamp");
        assert_eq!(
            ImportIssue::from(issue).to_string(),
            "page 3 skipped: missing timestamp"
        );
    }

    #[test]
    fn stream_error_messages_carry_offsets_and_limits() {
        assert_eq!(
            ExportError::Truncated { offset: 10 }.to_string(),
            "export is truncated (input ended at offset 10)"
        );
        assert_eq!(
            ExportError::LimitExceeded {
                limit: LimitKind::Page,
                max: 64
            }
            .to_string(),
            "export exceeds the page size limit of 64"
        );
        assert_eq!(
            ExportError::Read {
                kind: io::ErrorKind::UnexpectedEof
            }
            .to_string(),
            "reading the export failed: unexpected end of file"
        );
    }
}
