//! Reads one `<page>` element and validates it into an [`ImportedPage`].
//!
//! Reading and validating are separate so that a bad page is still read to its
//! end: the stream stays in sync and the next page can be imported. Reading
//! records the page's raw fields; validating decides whether they make a page.

use std::io::BufRead;

use super::error::{ExportError, PageField, PageIssueKind};
use super::tokens::{FieldContent, Token, TokenReader};
use crate::wiki::{ExportedHash, ImportedPage, PageId, PageTitle, RevisionId, RevisionTimestamp};

const WIKITEXT_MODEL: &str = "wikitext";
const WIKITEXT_FORMAT: &str = "text/x-wiki";
/// Longest value read for any field other than the wikitext. Real values are
/// far shorter; this only bounds what is buffered before validation rejects
/// them.
const MAX_SMALL_FIELD_BYTES: usize = 4096;

/// The fields of one page as read, before validation.
#[derive(Debug, Default)]
pub(super) struct RawPage {
    title: Option<FieldContent>,
    namespace: Option<FieldContent>,
    id: Option<FieldContent>,
    redirect: Redirect,
    revision: Option<RawRevision>,
    /// The first structural problem met while reading, which takes precedence
    /// over field validation.
    reading_issue: Option<PageIssueKind>,
}

#[derive(Debug, Default)]
struct RawRevision {
    id: Option<FieldContent>,
    timestamp: Option<FieldContent>,
    model: Option<FieldContent>,
    format: Option<FieldContent>,
    text: Option<FieldContent>,
    hash: Option<FieldContent>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum Redirect {
    #[default]
    Absent,
    Target(String),
    /// A `redirect` element without a usable `title` attribute.
    Invalid,
}

/// Reads the content of a `<page>` element whose start tag was just read, up to
/// and including its end tag.
///
/// # Errors
///
/// Returns an [`ExportError`] only for problems that make the rest of the
/// stream unreliable. Problems confined to this page are kept in the result and
/// reported by [`RawPage::into_page`].
pub(super) fn read_page<R: BufRead>(
    tokens: &mut TokenReader<R>,
    max_text_bytes: usize,
) -> Result<RawPage, ExportError> {
    let mut reader = PageReader {
        tokens,
        max_text_bytes,
        issue: None,
    };
    let mut page = reader.read_page_elements()?;
    page.reading_issue = reader.issue;
    Ok(page)
}

/// Reads the elements of one page, noting structural problems as it goes.
struct PageReader<'t, R> {
    tokens: &'t mut TokenReader<R>,
    max_text_bytes: usize,
    /// The first structural problem met, which the user should fix first.
    issue: Option<PageIssueKind>,
}

impl<R: BufRead> PageReader<'_, R> {
    fn read_page_elements(&mut self) -> Result<RawPage, ExportError> {
        let mut page = RawPage::default();
        loop {
            match self.tokens.next_token()? {
                Token::Start(element) => match element.export_name() {
                    Some("title") => self.read_small(&mut page.title, PageField::Title)?,
                    Some("ns") => self.read_small(&mut page.namespace, PageField::Namespace)?,
                    Some("id") => self.read_small(&mut page.id, PageField::PageId)?,
                    Some("redirect") => {
                        self.record_redirect(&mut page.redirect, element.attribute("title"));
                        self.tokens.skip_element()?;
                    }
                    Some("revision") => self.read_revision_once(&mut page.revision)?,
                    _ => self.tokens.skip_element()?,
                },
                Token::Text(_) => {}
                Token::End => return Ok(page),
                Token::Eof => return Err(self.tokens.structure_error()),
            }
        }
    }

    fn read_revision_once(&mut self, slot: &mut Option<RawRevision>) -> Result<(), ExportError> {
        if slot.is_some() {
            self.note(PageIssueKind::MultipleRevisions);
            return self.tokens.skip_element();
        }
        *slot = Some(self.read_revision()?);
        Ok(())
    }

    fn read_revision(&mut self) -> Result<RawRevision, ExportError> {
        let mut revision = RawRevision::default();
        loop {
            match self.tokens.next_token()? {
                Token::Start(element) => match element.export_name() {
                    Some("id") => self.read_small(&mut revision.id, PageField::RevisionId)?,
                    Some("timestamp") => {
                        self.read_small(&mut revision.timestamp, PageField::Timestamp)?;
                    }
                    Some("model") => self.read_small(&mut revision.model, PageField::Model)?,
                    Some("format") => self.read_small(&mut revision.format, PageField::Format)?,
                    Some("sha1") => self.read_small(&mut revision.hash, PageField::Hash)?,
                    // Deleted text stays absent rather than empty.
                    Some("text") if element.attribute("deleted").is_some() => {
                        self.tokens.skip_element()?;
                    }
                    Some("text") => {
                        let limit = self.max_text_bytes;
                        self.read_content(&mut revision.text, PageField::Text, limit)?;
                    }
                    _ => self.tokens.skip_element()?,
                },
                Token::Text(_) => {}
                Token::End => return Ok(revision),
                Token::Eof => return Err(self.tokens.structure_error()),
            }
        }
    }

    fn record_redirect(&mut self, slot: &mut Redirect, target: Option<&str>) {
        if *slot == Redirect::Absent {
            *slot = target.map_or(Redirect::Invalid, |target| {
                Redirect::Target(target.to_owned())
            });
        } else {
            self.note(PageIssueKind::DuplicateField(PageField::Redirect));
        }
    }

    fn read_small(
        &mut self,
        slot: &mut Option<FieldContent>,
        field: PageField,
    ) -> Result<(), ExportError> {
        self.read_content(slot, field, MAX_SMALL_FIELD_BYTES)
    }

    /// Reads an element into `slot`, noting a repeated element instead of
    /// overwriting the first.
    fn read_content(
        &mut self,
        slot: &mut Option<FieldContent>,
        field: PageField,
        max_bytes: usize,
    ) -> Result<(), ExportError> {
        if slot.is_some() {
            self.note(PageIssueKind::DuplicateField(field));
            return self.tokens.skip_element();
        }
        *slot = Some(self.tokens.read_field(max_bytes)?);
        Ok(())
    }

    fn note(&mut self, kind: PageIssueKind) {
        self.issue.get_or_insert(kind);
    }
}

impl RawPage {
    /// The page identifier, if it was read and is valid, for locating issues.
    pub(super) fn page_id(&self) -> Option<PageId> {
        match &self.id {
            Some(FieldContent::Text(text)) => parse_decimal(text).and_then(PageId::new),
            _ => None,
        }
    }

    /// Validates the raw fields into a page.
    ///
    /// Problems are reported in this order: anything noted while reading, then
    /// the title, namespace, page id, revision, revision id, timestamp,
    /// content type, text, hash and redirect.
    ///
    /// # Errors
    ///
    /// Returns the first problem as a [`PageIssueKind`].
    pub(super) fn into_page(self) -> Result<ImportedPage, PageIssueKind> {
        if let Some(kind) = self.reading_issue {
            return Err(kind);
        }
        let title = text_of(self.title, PageField::Title)?;
        let title = PageTitle::new(title).ok_or(PageIssueKind::InvalidField(PageField::Title))?;
        let namespace = number_of(self.namespace, PageField::Namespace)?;
        let namespace = u32::try_from(namespace)
            .map_err(|_| PageIssueKind::InvalidField(PageField::Namespace))?;
        let id = number_of(self.id, PageField::PageId)?;
        let id = PageId::new(id).ok_or(PageIssueKind::InvalidField(PageField::PageId))?;
        let revision = self.revision.ok_or(PageIssueKind::MissingRevision)?;
        let (revision_id, timestamp, hash, text) = revision.into_parts()?;
        let redirect_target = redirect_target(self.redirect)?;
        Ok(ImportedPage {
            id,
            namespace,
            title,
            revision: revision_id,
            timestamp,
            hash,
            redirect_target,
            text,
        })
    }
}

impl RawRevision {
    fn into_parts(
        self,
    ) -> Result<(RevisionId, RevisionTimestamp, Option<ExportedHash>, String), PageIssueKind> {
        let id = number_of(self.id, PageField::RevisionId)?;
        let id = RevisionId::new(id).ok_or(PageIssueKind::InvalidField(PageField::RevisionId))?;
        let timestamp = text_of(self.timestamp, PageField::Timestamp)?;
        let timestamp = RevisionTimestamp::parse(&timestamp)
            .map_err(|_| PageIssueKind::InvalidField(PageField::Timestamp))?;
        require_wikitext(self.model, self.format)?;
        let text = text_of(self.text, PageField::Text)?;
        if text.trim().is_empty() {
            return Err(PageIssueKind::EmptyText);
        }
        let hash = self
            .hash
            .map(|content| {
                let hash = text_of(Some(content), PageField::Hash)?;
                ExportedHash::new(hash).ok_or(PageIssueKind::InvalidField(PageField::Hash))
            })
            .transpose()?;
        Ok((id, timestamp, hash, text))
    }
}

/// The text of a field, or why it is not usable.
fn text_of(content: Option<FieldContent>, field: PageField) -> Result<String, PageIssueKind> {
    match content {
        None => Err(PageIssueKind::MissingField(field)),
        Some(FieldContent::Text(text)) => Ok(text),
        Some(FieldContent::TooLong) if field == PageField::Text => {
            Err(PageIssueKind::TextOverLimit)
        }
        Some(FieldContent::TooLong | FieldContent::Nested) => {
            Err(PageIssueKind::InvalidField(field))
        }
    }
}

fn number_of(content: Option<FieldContent>, field: PageField) -> Result<u64, PageIssueKind> {
    parse_decimal(&text_of(content, field)?).ok_or(PageIssueKind::InvalidField(field))
}

/// A non-negative decimal integer written only with ASCII digits.
fn parse_decimal(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn redirect_target(redirect: Redirect) -> Result<Option<PageTitle>, PageIssueKind> {
    let invalid = PageIssueKind::InvalidField(PageField::Redirect);
    match redirect {
        Redirect::Absent => Ok(None),
        Redirect::Invalid => Err(invalid),
        Redirect::Target(target) => PageTitle::new(target).map(Some).ok_or(invalid),
    }
}

/// Accepts a missing model or format, since the content is then the wiki's
/// default, and refuses a stated one that is not wikitext.
fn require_wikitext(
    model: Option<FieldContent>,
    format: Option<FieldContent>,
) -> Result<(), PageIssueKind> {
    require_value(model, PageField::Model, WIKITEXT_MODEL)?;
    require_value(format, PageField::Format, WIKITEXT_FORMAT)
}

fn require_value(
    content: Option<FieldContent>,
    field: PageField,
    expected: &str,
) -> Result<(), PageIssueKind> {
    let Some(content) = content else {
        return Ok(());
    };
    if text_of(Some(content), field)? == expected {
        Ok(())
    } else {
        Err(PageIssueKind::UnsupportedContent)
    }
}
