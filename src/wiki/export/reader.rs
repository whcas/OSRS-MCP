//! The importer's reading loop: header, then pages in file order.

use std::collections::HashSet;
use std::fmt;
use std::io::BufRead;

use super::error::{ExportError, ImportIssue, PageIssue, PageIssueKind};
use super::tokens::{Token, TokenReader};
use super::{ExportSummary, ImportLimits, header, page};
use crate::wiki::{ImportedPage, PageId, SourceId};

/// Reads pages from a `MediaWiki` export. See the [module documentation](self)
/// for the supported format and the error model.
///
/// The iterator yields pages in file order. After it yields an
/// [`ImportIssue::Stream`] or returns `None`, it yields nothing more.
pub struct ExportReader<R> {
    tokens: TokenReader<R>,
    source: SourceId,
    limits: ImportLimits,
    seen_pages: HashSet<PageId>,
    ordinal: u64,
    summary: ExportSummary,
    finished: bool,
}

impl<R> fmt::Debug for ExportReader<R> {
    /// Omits the parser's buffers, which hold text from the export.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExportReader")
            .field("source", &self.source)
            .field("pages_read", &self.ordinal)
            .field("summary", &self.summary)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl<R: BufRead> ExportReader<R> {
    /// Reads the export header and returns a reader positioned at the first
    /// page.
    ///
    /// # Errors
    ///
    /// Returns an [`ExportError`] if the input is not a supported export, has
    /// no valid `siteinfo` header, or cannot be read.
    pub fn open(input: R, limits: ImportLimits) -> Result<Self, ExportError> {
        let mut tokens =
            TokenReader::new(input, limits.max_input_bytes.get(), limits.max_depth.get());
        let source = header::read_header(&mut tokens)?;
        Ok(Self {
            tokens,
            source,
            limits,
            seen_pages: HashSet::new(),
            ordinal: 0,
            summary: ExportSummary::default(),
            finished: false,
        })
    }

    /// The wiki this export came from.
    #[must_use]
    pub const fn source(&self) -> &SourceId {
        &self.source
    }

    /// Counts of what has been read so far. Complete once the iterator is
    /// exhausted.
    #[must_use]
    pub const fn summary(&self) -> &ExportSummary {
        &self.summary
    }

    /// Reads to the next page element, then reads and validates it. `None`
    /// means the root element closed.
    fn next_page(&mut self) -> Result<Option<Result<ImportedPage, PageIssue>>, ExportError> {
        loop {
            match self.tokens.next_token()? {
                Token::Start(element) if element.export_name() == Some("page") => {
                    self.ordinal += 1;
                    return self.read_page().map(Some);
                }
                Token::Start(_) => self.tokens.skip_element()?,
                Token::Text(text) if text.trim().is_empty() => {}
                Token::Text(_) => return Err(self.tokens.structure_error()),
                Token::End | Token::Eof => return Ok(None),
            }
        }
    }

    fn read_page(&mut self) -> Result<Result<ImportedPage, PageIssue>, ExportError> {
        self.tokens.begin_page(self.limits.max_page_bytes.get());
        let raw = page::read_page(&mut self.tokens, self.limits.max_text_bytes.get())?;
        self.tokens.end_page();

        let page_id = raw.page_id();
        let validated = raw.into_page().and_then(|page| self.reject_duplicate(page));
        Ok(validated.map_err(|kind| PageIssue {
            ordinal: self.ordinal,
            page_id,
            kind,
        }))
    }

    fn reject_duplicate(&mut self, page: ImportedPage) -> Result<ImportedPage, PageIssueKind> {
        if self.seen_pages.insert(page.id) {
            Ok(page)
        } else {
            Err(PageIssueKind::DuplicatePage)
        }
    }
}

impl<R: BufRead> Iterator for ExportReader<R> {
    type Item = Result<ImportedPage, ImportIssue>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.next_page() {
            Ok(Some(Ok(page))) => {
                self.summary.record_import(&page);
                Some(Ok(page))
            }
            Ok(Some(Err(issue))) => {
                self.summary.record_skip(&issue);
                Some(Err(ImportIssue::Page(issue)))
            }
            Ok(None) => {
                self.finished = true;
                None
            }
            Err(error) => {
                self.finished = true;
                self.summary.stream_error = Some(error.clone());
                Some(Err(ImportIssue::Stream(error)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fmt;
    use std::io::{self, BufReader, Read};
    use std::num::{NonZeroU64, NonZeroUsize};
    use std::rc::Rc;

    use super::super::error::{LimitKind, PageField, XmlFault};
    use super::*;
    use crate::wiki::{ExportedHash, PageTitle, SourceField};

    const NAMESPACE: &str = "http://www.mediawiki.org/xml/export-0.11/";
    const SITEINFO: &str = "<siteinfo><sitename>Example Wiki</sitename>\
        <dbname>examplewiki</dbname><base>https://wiki.example/w/Main_Page</base>\
        <generator>MediaWiki 1.45.3</generator><case>first-letter</case>\
        <namespaces><namespace key=\"0\" case=\"first-letter\" /></namespaces></siteinfo>";

    /// Changes one aspect of a fixture page.
    type Mutation = fn(&mut PageFixture);

    /// One synthetic page. `None` omits the element, so a test changes only
    /// what it is about.
    #[derive(Clone)]
    struct PageFixture {
        id: Option<String>,
        title: Option<String>,
        namespace: Option<String>,
        redirect: Option<String>,
        revision_id: Option<String>,
        timestamp: Option<String>,
        model: Option<String>,
        format: Option<String>,
        hash: Option<String>,
        /// Written between the `<text>` tags exactly as given, so tests
        /// control the escaping.
        text: Option<String>,
    }

    impl PageFixture {
        fn new(id: u64, title: &str) -> Self {
            Self {
                id: Some(id.to_string()),
                title: Some(title.to_owned()),
                namespace: Some("0".to_owned()),
                redirect: None,
                revision_id: Some((id * 100).to_string()),
                timestamp: Some("2026-03-04T05:06:07Z".to_owned()),
                model: Some("wikitext".to_owned()),
                format: Some("text/x-wiki".to_owned()),
                hash: None,
                text: Some(format!("Synthetic text for page {id}.")),
            }
        }

        fn with_text(mut self, text: &str) -> Self {
            self.text = Some(text.to_owned());
            self
        }

        fn xml(&self) -> String {
            fn element(name: &str, value: Option<&String>) -> String {
                value.map_or_else(String::new, |value| format!("<{name}>{value}</{name}>"))
            }
            let redirect = self.redirect.as_ref().map_or_else(String::new, |target| {
                format!("<redirect title=\"{target}\" />")
            });
            let text = self.text.as_ref().map_or_else(String::new, |text| {
                format!(
                    "<text bytes=\"{}\" xml:space=\"preserve\">{text}</text>",
                    text.len()
                )
            });
            format!(
                "<page>{}{}{}{redirect}<revision>{}<parentid>1</parentid>{}\
                 <contributor><username>Synthetic editor</username><id>9</id></contributor>\
                 <comment>synthetic edit</comment>{}{}{text}{}</revision></page>",
                element("title", self.title.as_ref()),
                element("ns", self.namespace.as_ref()),
                element("id", self.id.as_ref()),
                element("id", self.revision_id.as_ref()),
                element("timestamp", self.timestamp.as_ref()),
                element("model", self.model.as_ref()),
                element("format", self.format.as_ref()),
                element("sha1", self.hash.as_ref()),
            )
        }
    }

    fn export_of(pages: &[PageFixture]) -> String {
        let body: String = pages.iter().map(PageFixture::xml).collect();
        wrap(SITEINFO, &body)
    }

    fn wrap(siteinfo: &str, body: &str) -> String {
        format!(
            "<mediawiki xmlns=\"{NAMESPACE}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
             xsi:schemaLocation=\"{NAMESPACE} http://www.mediawiki.org/xml/export-0.11.xsd\" \
             version=\"0.11\" xml:lang=\"en\">{siteinfo}{body}</mediawiki>"
        )
    }

    fn limits(input: u64, page: u64, text: usize, depth: usize) -> ImportLimits {
        ImportLimits {
            max_input_bytes: NonZeroU64::new(input).unwrap(),
            max_page_bytes: NonZeroU64::new(page).unwrap(),
            max_text_bytes: NonZeroUsize::new(text).unwrap(),
            max_depth: NonZeroUsize::new(depth).unwrap(),
        }
    }

    struct Imported {
        items: Vec<Result<ImportedPage, ImportIssue>>,
        summary: ExportSummary,
    }

    impl Imported {
        fn pages(&self) -> Vec<&ImportedPage> {
            self.items
                .iter()
                .filter_map(|item| item.as_ref().ok())
                .collect()
        }

        fn issues(&self) -> Vec<&ImportIssue> {
            self.items
                .iter()
                .filter_map(|item| item.as_ref().err())
                .collect()
        }

        fn only_issue(&self) -> &ImportIssue {
            let issues = self.issues();
            assert_eq!(issues.len(), 1, "expected exactly one issue");
            issues[0]
        }
    }

    fn import_bytes(xml: &[u8], limits: ImportLimits) -> Result<Imported, ExportError> {
        let mut reader = ExportReader::open(xml, limits)?;
        let items: Vec<_> = reader.by_ref().collect();
        Ok(Imported {
            items,
            summary: reader.summary().clone(),
        })
    }

    fn import(xml: &str) -> Imported {
        import_bytes(xml.as_bytes(), ImportLimits::default()).unwrap()
    }

    fn open_error(xml: &str) -> ExportError {
        ExportReader::open(xml.as_bytes(), ImportLimits::default()).unwrap_err()
    }

    /// The kind of the only issue when a single page is skipped.
    fn skipped_kind(page: &PageFixture) -> PageIssueKind {
        match import(&export_of(std::slice::from_ref(page))).only_issue() {
            ImportIssue::Page(issue) => issue.kind,
            ImportIssue::Stream(error) => panic!("unexpected stream error: {error}"),
        }
    }

    fn stream_error(imported: &Imported) -> &ExportError {
        match imported.only_issue() {
            ImportIssue::Stream(error) => error,
            ImportIssue::Page(issue) => panic!("unexpected page issue: {issue}"),
        }
    }

    // --- Well-formed pages ---

    #[test]
    fn imports_every_field_of_a_prose_page() {
        let mut fixture = PageFixture::new(12, "Example subject");
        fixture.namespace = Some("0".to_owned());
        fixture.hash = Some("abc123def456".to_owned());
        fixture.text =
            Some("'''Example subject''' is a made-up topic.\n\n== Facts ==\n* One".to_owned());
        let imported = import(&export_of(&[fixture]));

        let page = imported.pages()[0];
        assert_eq!(page.id.get(), 12);
        assert_eq!(page.namespace, 0);
        assert_eq!(page.title.as_str(), "Example subject");
        assert_eq!(page.revision.get(), 1200);
        assert_eq!(page.timestamp.as_str(), "2026-03-04T05:06:07Z");
        assert_eq!(page.timestamp.seconds().0, 1_772_600_767);
        assert_eq!(
            page.hash.as_ref().map(ExportedHash::as_str),
            Some("abc123def456")
        );
        assert_eq!(page.redirect_target, None);
        assert_eq!(
            page.text,
            "'''Example subject''' is a made-up topic.\n\n== Facts ==\n* One"
        );
        assert_eq!(imported.summary.pages_imported, 1);
    }

    #[test]
    fn reports_the_site_identity_from_the_header() {
        let xml = export_of(&[]);
        let reader = ExportReader::open(xml.as_bytes(), ImportLimits::default()).unwrap();
        let source = reader.source();
        assert_eq!(source.site_name(), "Example Wiki");
        assert_eq!(source.db_name(), Some("examplewiki"));
        assert_eq!(source.base_url(), "https://wiki.example/w/Main_Page");
    }

    #[test]
    fn pages_come_back_in_file_order() {
        let pages = [
            PageFixture::new(30, "C"),
            PageFixture::new(10, "A"),
            PageFixture::new(20, "B"),
        ];
        let imported = import(&export_of(&pages));
        let ids: Vec<u64> = imported.pages().iter().map(|page| page.id.get()).collect();
        assert_eq!(ids, [30, 10, 20]);
    }

    #[test]
    fn a_redirect_records_its_target_and_keeps_its_text() {
        let mut fixture = PageFixture::new(5, "Old name").with_text("#REDIRECT [[New name]]");
        fixture.redirect = Some("New name".to_owned());
        let imported = import(&export_of(&[fixture]));

        let page = imported.pages()[0];
        assert_eq!(page.redirect_target, PageTitle::new("New name".to_owned()));
        assert_eq!(page.text, "#REDIRECT [[New name]]");
        assert_eq!(imported.summary.redirects_imported, 1);
    }

    #[test]
    fn a_redirect_target_outside_the_export_is_not_invented() {
        let mut fixture = PageFixture::new(5, "Old name").with_text("#REDIRECT [[Absent page]]");
        fixture.redirect = Some("Absent page".to_owned());
        let imported = import(&export_of(&[fixture]));
        assert_eq!(imported.pages().len(), 1);
        assert_eq!(imported.summary.pages_imported, 1);
    }

    #[test]
    fn nested_templates_and_tables_are_kept_verbatim() {
        let text = "{{Infobox\n|name = Example\n|value = {{Formula|{{Count|3}}|x}}\n}}\n\
                    {| class=\"wikitable\"\n! Head\n|-\n| {{Cell|1}} || 2\n|}";
        let imported = import(&export_of(&[PageFixture::new(1, "Layout").with_text(text)]));
        assert_eq!(imported.pages()[0].text, text);
    }

    #[test]
    fn xml_escapes_and_character_references_are_resolved() {
        let escaped =
            "a &lt;ref&gt;x&lt;/ref&gt; &amp; &quot;b&quot; &apos;c&apos; caf&#233; &#x1F41F;";
        let imported = import(&export_of(&[
            PageFixture::new(1, "Escapes").with_text(escaped)
        ]));
        assert_eq!(
            imported.pages()[0].text,
            "a <ref>x</ref> & \"b\" 'c' café 🐟"
        );
    }

    #[test]
    fn cdata_is_taken_literally() {
        let text = "<![CDATA[a <b> & {{c}} ]]>";
        let imported = import(&export_of(&[PageFixture::new(1, "Cdata").with_text(text)]));
        assert_eq!(imported.pages()[0].text, "a <b> & {{c}} ");
    }

    #[test]
    fn line_endings_are_normalised_as_xml_requires() {
        let imported = import(&export_of(&[
            PageFixture::new(1, "Lines").with_text("one\r\ntwo\rthree\nfour")
        ]));
        assert_eq!(imported.pages()[0].text, "one\ntwo\nthree\nfour");
    }

    #[test]
    fn unicode_survives_input_that_is_read_in_tiny_chunks() {
        let text = "Zürich ☕ 日本語 🐟🐠 Ελληνικά";
        let xml = export_of(&[PageFixture::new(1, "Ünïcode ☕").with_text(text)]);
        // Three bytes per read splits every multi-byte character somewhere.
        let reader = BufReader::with_capacity(3, xml.as_bytes());
        let pages: Vec<_> = ExportReader::open(reader, ImportLimits::default())
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(pages[0].title.as_str(), "Ünïcode ☕");
        assert_eq!(pages[0].text, text);
    }

    #[test]
    fn contributor_comment_and_other_elements_are_ignored() {
        // The fixture writes a contributor and a comment into every revision.
        let imported = import(&export_of(&[PageFixture::new(1, "Quiet")]));
        let debug = format!("{:?}", imported.pages()[0]);
        assert!(!debug.contains("Synthetic editor"));
        assert!(!debug.contains("synthetic edit"));
        assert_eq!(imported.summary.pages_imported, 1);
    }

    #[test]
    fn unknown_elements_and_elements_in_other_namespaces_are_skipped() {
        let page = "<page><title>Real</title><ns>0</ns><id>7</id>\
            <x:title xmlns:x=\"urn:example:other\">Impostor</x:title>\
            <restrictions>edit=sysop</restrictions><x:ext xmlns:x=\"urn:example:other\"><a><b/></a></x:ext>\
            <revision><id>70</id><timestamp>2026-01-02T03:04:05Z</timestamp>\
            <text>Body</text></revision></page>";
        let imported = import(&wrap(SITEINFO, page));
        assert_eq!(imported.pages()[0].title.as_str(), "Real");
    }

    #[test]
    fn comments_processing_instructions_and_whitespace_between_pages_are_ignored() {
        let page = PageFixture::new(1, "One").xml();
        let body = format!("\n  <!-- note -->\n  <?tool data?>\n  {page}\n  ");
        let imported = import(&wrap(SITEINFO, &body));
        assert_eq!(imported.pages().len(), 1);
    }

    #[test]
    fn content_after_the_closing_root_tag_is_ignored() {
        let xml = format!("{}trailing", export_of(&[PageFixture::new(1, "One")]));
        assert_eq!(import(&xml).pages().len(), 1);
    }

    // --- Pages that are skipped ---

    #[test]
    fn a_skipped_page_does_not_hide_the_pages_around_it() {
        let mut broken = PageFixture::new(2, "Broken");
        broken.timestamp = None;
        let imported = import(&export_of(&[
            PageFixture::new(1, "A"),
            broken,
            PageFixture::new(3, "C"),
        ]));

        assert_eq!(imported.pages().len(), 2);
        assert_eq!(imported.summary.pages_imported, 2);
        assert_eq!(imported.summary.total_pages_skipped(), 1);
        assert_eq!(imported.summary.stream_error, None);
    }

    #[test]
    fn an_issue_locates_the_page_by_ordinal_and_id_without_its_title() {
        let mut broken = PageFixture::new(42, "Secret title");
        broken.text = None;
        let imported = import(&export_of(&[PageFixture::new(1, "A"), broken]));

        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.ordinal, 2);
        assert_eq!(issue.page_id.map(PageId::get), Some(42));
        assert!(!issue.to_string().contains("Secret"));
    }

    #[test]
    fn each_required_element_is_reported_when_absent() {
        let cases: [(Mutation, PageField); 5] = [
            (|page| page.title = None, PageField::Title),
            (|page| page.namespace = None, PageField::Namespace),
            (|page| page.id = None, PageField::PageId),
            (|page| page.revision_id = None, PageField::RevisionId),
            (|page| page.timestamp = None, PageField::Timestamp),
        ];
        for (remove, field) in cases {
            let mut page = PageFixture::new(1, "A");
            remove(&mut page);
            assert_eq!(
                skipped_kind(&page),
                PageIssueKind::MissingField(field),
                "{field}"
            );
        }
    }

    #[test]
    fn invalid_values_are_reported_against_their_field() {
        let cases: [(Mutation, PageField); 9] = [
            (
                |page| page.namespace = Some("main".to_owned()),
                PageField::Namespace,
            ),
            (
                |page| page.namespace = Some("-1".to_owned()),
                PageField::Namespace,
            ),
            (|page| page.id = Some("0".to_owned()), PageField::PageId),
            (|page| page.id = Some("+5".to_owned()), PageField::PageId),
            (
                |page| page.revision_id = Some("12x".to_owned()),
                PageField::RevisionId,
            ),
            (
                |page| page.timestamp = Some("yesterday".to_owned()),
                PageField::Timestamp,
            ),
            (|page| page.title = Some("  ".to_owned()), PageField::Title),
            (
                |page| page.hash = Some("NOT-A-HASH".to_owned()),
                PageField::Hash,
            ),
            (
                |page| page.redirect = Some(String::new()),
                PageField::Redirect,
            ),
        ];
        for (corrupt, field) in cases {
            let mut page = PageFixture::new(1, "A");
            corrupt(&mut page);
            assert_eq!(
                skipped_kind(&page),
                PageIssueKind::InvalidField(field),
                "{field}"
            );
        }
    }

    #[test]
    fn a_redirect_without_a_target_is_invalid() {
        let page = "<page><title>A</title><ns>0</ns><id>1</id><redirect />\
            <revision><id>10</id><timestamp>2026-01-02T03:04:05Z</timestamp>\
            <text>#REDIRECT [[B]]</text></revision></page>";
        let imported = import(&wrap(SITEINFO, page));
        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::InvalidField(PageField::Redirect));
    }

    #[test]
    fn a_page_without_a_revision_is_reported() {
        let page = "<page><title>A</title><ns>0</ns><id>1</id></page>";
        let imported = import(&wrap(
            SITEINFO,
            &format!("{page}{}", PageFixture::new(2, "B").xml()),
        ));

        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::MissingRevision);
        assert_eq!(
            imported.pages().len(),
            1,
            "the following page is still imported"
        );
    }

    #[test]
    fn a_page_with_several_revisions_is_refused_as_history() {
        let page = PageFixture::new(1, "A").xml();
        let history = page.replace("</page>", &format!("{}</page>", revision_of(&page)));
        let imported = import(&wrap(SITEINFO, &history));

        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::MultipleRevisions);
    }

    fn revision_of(page: &str) -> &str {
        let start = page.find("<revision>").unwrap();
        let end = page.find("</revision>").unwrap() + "</revision>".len();
        &page[start..end]
    }

    #[test]
    fn missing_empty_blank_and_deleted_text_are_distinguished() {
        let mut absent = PageFixture::new(1, "A");
        absent.text = None;
        assert_eq!(
            skipped_kind(&absent),
            PageIssueKind::MissingField(PageField::Text)
        );

        assert_eq!(
            skipped_kind(&PageFixture::new(1, "A").with_text("")),
            PageIssueKind::EmptyText
        );
        assert_eq!(
            skipped_kind(&PageFixture::new(1, "A").with_text(" \n\t ")),
            PageIssueKind::EmptyText
        );

        let deleted = "<page><title>A</title><ns>0</ns><id>1</id><revision><id>10</id>\
            <timestamp>2026-01-02T03:04:05Z</timestamp><text deleted=\"deleted\" /></revision></page>";
        let imported = import(&wrap(SITEINFO, deleted));
        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::MissingField(PageField::Text));
    }

    #[test]
    fn an_empty_text_element_is_empty_not_missing() {
        let empty = "<page><title>A</title><ns>0</ns><id>1</id><revision><id>10</id>\
            <timestamp>2026-01-02T03:04:05Z</timestamp><text bytes=\"0\" xml:space=\"preserve\" />\
            </revision></page>";
        let imported = import(&wrap(SITEINFO, empty));
        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::EmptyText);
    }

    #[test]
    fn content_that_is_not_wikitext_is_unsupported() {
        for (model, format) in [
            ("Scribunto", "text/plain"),
            ("wikitext", "text/plain"),
            ("json", "text/x-wiki"),
        ] {
            let mut page = PageFixture::new(1, "Module:Example");
            page.model = Some(model.to_owned());
            page.format = Some(format.to_owned());
            assert_eq!(
                skipped_kind(&page),
                PageIssueKind::UnsupportedContent,
                "{model} {format}"
            );
        }
    }

    #[test]
    fn a_missing_model_and_format_are_taken_to_be_wikitext() {
        let mut page = PageFixture::new(1, "A");
        page.model = None;
        page.format = None;
        assert_eq!(import(&export_of(&[page])).pages().len(), 1);
    }

    #[test]
    fn repeated_elements_are_reported_and_the_first_is_not_overwritten() {
        let page = PageFixture::new(1, "A").xml();
        let twice_title = page.replace("<title>A</title>", "<title>A</title><title>B</title>");
        let twice_stamp = page.replace(
            "<comment>",
            "<timestamp>2026-01-01T00:00:00Z</timestamp><comment>",
        );
        let twice_redirect = page.replace(
            "<revision>",
            "<redirect title=\"X\" /><redirect title=\"Y\" /><revision>",
        );
        for (xml, field) in [
            (twice_title, PageField::Title),
            (twice_stamp, PageField::Timestamp),
            (twice_redirect, PageField::Redirect),
        ] {
            let imported = import(&wrap(SITEINFO, &xml));
            let ImportIssue::Page(issue) = imported.only_issue() else {
                panic!("expected a page issue");
            };
            assert_eq!(issue.kind, PageIssueKind::DuplicateField(field));
        }
    }

    #[test]
    fn a_repeated_page_id_keeps_the_first_page() {
        let pages = [
            PageFixture::new(9, "First"),
            PageFixture::new(9, "Second"),
            PageFixture::new(10, "Third"),
        ];
        let imported = import(&export_of(&pages));

        let titles: Vec<&str> = imported
            .pages()
            .iter()
            .map(|page| page.title.as_str())
            .collect();
        assert_eq!(titles, ["First", "Third"]);
        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::DuplicatePage);
        assert_eq!(issue.ordinal, 2);
    }

    #[test]
    fn the_first_problem_in_field_order_is_the_one_reported() {
        let mut page = PageFixture::new(1, "A");
        page.title = None;
        page.timestamp = Some("bad".to_owned());
        page.text = None;
        assert_eq!(
            skipped_kind(&page),
            PageIssueKind::MissingField(PageField::Title)
        );
    }

    #[test]
    fn summary_counts_skipped_pages_by_reason() {
        let empty = PageFixture::new(1, "A").with_text("");
        let also_empty = PageFixture::new(2, "B").with_text(" ");
        let mut no_title = PageFixture::new(3, "C");
        no_title.title = None;
        let imported = import(&export_of(&[
            empty,
            also_empty,
            no_title,
            PageFixture::new(4, "D"),
        ]));

        assert_eq!(
            imported
                .summary
                .pages_skipped
                .get(&PageIssueKind::EmptyText),
            Some(&2)
        );
        assert_eq!(
            imported
                .summary
                .pages_skipped
                .get(&PageIssueKind::MissingField(PageField::Title)),
            Some(&1)
        );
        assert_eq!(imported.summary.total_pages_skipped(), 3);
        assert_eq!(imported.summary.pages_imported, 1);
    }

    // --- Header and schema ---

    #[test]
    fn other_schema_versions_are_unsupported() {
        let old_namespace = "http://www.mediawiki.org/xml/export-0.10/";
        let cases = [
            format!("<mediawiki xmlns=\"{old_namespace}\" version=\"0.10\">{SITEINFO}</mediawiki>"),
            format!("<mediawiki xmlns=\"{NAMESPACE}\" version=\"0.10\">{SITEINFO}</mediawiki>"),
            format!("<mediawiki xmlns=\"{NAMESPACE}\">{SITEINFO}</mediawiki>"),
            format!("<mediawiki version=\"0.11\">{SITEINFO}</mediawiki>"),
        ];
        for xml in cases {
            assert_eq!(open_error(&xml), ExportError::UnsupportedSchema, "{xml}");
        }
    }

    #[test]
    fn input_that_is_not_an_export_is_refused() {
        for xml in [
            "",
            "   ",
            "plain text",
            "<html><body/></html>",
            "<mediawikis/>",
        ] {
            assert_eq!(open_error(xml), ExportError::NotAnExport, "{xml:?}");
        }
    }

    #[test]
    fn a_missing_siteinfo_is_reported() {
        let page = PageFixture::new(1, "A").xml();
        assert_eq!(open_error(&wrap("", &page)), ExportError::MissingSiteInfo);
        assert_eq!(open_error(&wrap("", "")), ExportError::MissingSiteInfo);
    }

    #[test]
    fn a_siteinfo_without_a_site_name_or_base_url_is_invalid() {
        let no_name = "<siteinfo><base>https://wiki.example/</base></siteinfo>";
        let no_base = "<siteinfo><sitename>Example</sitename></siteinfo>";
        assert_eq!(
            open_error(&wrap(no_name, "")),
            ExportError::InvalidSiteInfo {
                field: SourceField::SiteName
            }
        );
        assert_eq!(
            open_error(&wrap(no_base, "")),
            ExportError::InvalidSiteInfo {
                field: SourceField::BaseUrl
            }
        );
    }

    #[test]
    fn a_siteinfo_with_a_repeated_or_unsafe_field_is_invalid() {
        let repeated = "<siteinfo><sitename>A</sitename><sitename>B</sitename>\
            <base>https://wiki.example/</base></siteinfo>";
        let unsafe_url =
            "<siteinfo><sitename>A</sitename><base>javascript:alert(1)</base></siteinfo>";
        assert_eq!(
            open_error(&wrap(repeated, "")),
            ExportError::InvalidSiteInfo {
                field: SourceField::SiteName
            }
        );
        assert_eq!(
            open_error(&wrap(unsafe_url, "")),
            ExportError::InvalidSiteInfo {
                field: SourceField::BaseUrl
            }
        );
    }

    #[test]
    fn the_database_name_is_optional() {
        let siteinfo =
            "<siteinfo><sitename>A</sitename><base>https://wiki.example/</base></siteinfo>";
        let xml = wrap(siteinfo, "");
        let reader = ExportReader::open(xml.as_bytes(), ImportLimits::default()).unwrap();
        assert_eq!(reader.source().db_name(), None);
    }

    // --- Refused or broken documents ---

    #[test]
    fn document_type_declarations_are_refused_before_anything_is_defined() {
        let internal = "<!DOCTYPE mediawiki [<!ENTITY boom \"expanded\">]>";
        let external = "<!DOCTYPE mediawiki SYSTEM \"http://127.0.0.1:1/export.dtd\">";
        for doctype in [internal, external] {
            let xml = format!("{doctype}{}", export_of(&[]));
            assert!(
                matches!(open_error(&xml), ExportError::DoctypeNotAllowed { .. }),
                "{doctype}"
            );
        }
    }

    #[test]
    fn an_undeclared_entity_is_malformed_and_never_expanded() {
        let imported = import(&export_of(&[
            PageFixture::new(1, "A").with_text("before &boom; after")
        ]));
        assert_eq!(
            stream_error(&imported),
            &ExportError::Malformed {
                fault: XmlFault::Reference,
                offset: stream_offset(&imported)
            }
        );
        assert_eq!(imported.pages().len(), 0);
    }

    fn stream_offset(imported: &Imported) -> u64 {
        match stream_error(imported) {
            ExportError::Malformed { offset, .. } => *offset,
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn an_encoding_other_than_utf8_is_unsupported() {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>{}",
            export_of(&[])
        );
        assert_eq!(open_error(&xml), ExportError::UnsupportedEncoding);
        let utf8 = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>{}",
            export_of(&[])
        );
        assert!(ExportReader::open(utf8.as_bytes(), ImportLimits::default()).is_ok());
    }

    #[test]
    fn bytes_that_are_not_utf8_are_malformed() {
        let mut xml = export_of(&[PageFixture::new(1, "A")]).into_bytes();
        let position = xml.windows(4).position(|window| window == b"Synt").unwrap();
        xml[position] = 0xFF;
        let imported = import_bytes(&xml, ImportLimits::default()).unwrap();
        assert!(matches!(
            stream_error(&imported),
            ExportError::Malformed {
                fault: XmlFault::Encoding,
                ..
            }
        ));
    }

    #[test]
    fn a_mismatched_end_tag_is_malformed() {
        let xml = export_of(&[PageFixture::new(1, "A")]).replace("</title>", "</titel>");
        let imported = import(&xml);
        assert!(matches!(
            stream_error(&imported),
            ExportError::Malformed {
                fault: XmlFault::TagMismatch,
                ..
            }
        ));
    }

    #[test]
    fn text_between_pages_is_a_structure_error() {
        let page = PageFixture::new(1, "A").xml();
        let imported = import(&wrap(SITEINFO, &format!("{page}stray text")));
        assert!(matches!(
            stream_error(&imported),
            ExportError::Malformed {
                fault: XmlFault::Structure,
                ..
            }
        ));
    }

    #[test]
    fn an_input_cut_short_reports_truncation_after_the_pages_that_were_complete() {
        let xml = export_of(&[PageFixture::new(1, "A"), PageFixture::new(2, "B")]);
        let cut_inside_second_text = xml.rfind("Synthetic text").unwrap() + 5;
        let imported = import_bytes(
            &xml.as_bytes()[..cut_inside_second_text],
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(imported.pages().len(), 1);
        assert!(matches!(
            stream_error(&imported),
            ExportError::Truncated { .. }
        ));
        assert_eq!(
            imported.summary.stream_error.as_ref(),
            Some(stream_error(&imported))
        );
    }

    #[test]
    fn truncation_inside_a_tag_or_before_the_closing_tag_is_reported() {
        let xml = export_of(&[PageFixture::new(1, "A")]);
        let inside_tag = xml.rfind("<revision>").unwrap() + 4;
        let before_root_end = xml.len() - "</mediawiki>".len();
        for end in [inside_tag, before_root_end] {
            let imported = import_bytes(&xml.as_bytes()[..end], ImportLimits::default()).unwrap();
            assert!(
                matches!(stream_error(&imported), ExportError::Truncated { .. }),
                "{end}"
            );
        }
    }

    #[test]
    fn a_truncated_header_is_reported_when_opening() {
        let xml = export_of(&[]);
        let cut = xml.find("</sitename>").unwrap();
        let error =
            ExportReader::open(&xml.as_bytes()[..cut], ImportLimits::default()).unwrap_err();
        assert!(matches!(error, ExportError::Truncated { .. }));
    }

    #[test]
    fn nothing_is_yielded_after_a_stream_error() {
        let xml = export_of(&[PageFixture::new(1, "A")]).replace("</title>", "</x>");
        let mut reader = ExportReader::open(xml.as_bytes(), ImportLimits::default()).unwrap();
        assert!(matches!(reader.next(), Some(Err(ImportIssue::Stream(_)))));
        assert!(reader.next().is_none());
        assert!(reader.next().is_none());
    }

    // --- Limits ---

    #[test]
    fn default_limits_are_ordered_so_that_text_fits_in_a_page_in_the_input() {
        let defaults = ImportLimits::default();
        assert!(defaults.max_text_bytes.get() as u64 <= defaults.max_page_bytes.get());
        assert!(defaults.max_page_bytes <= defaults.max_input_bytes);
        assert!(defaults.max_depth.get() > 5);
    }

    #[test]
    fn input_exactly_at_the_limit_is_accepted_and_one_byte_more_is_not() {
        let xml = export_of(&[PageFixture::new(1, "A")]);
        let length = xml.len() as u64;

        let at_limit = import_bytes(xml.as_bytes(), limits(length, 1 << 20, 1 << 20, 32)).unwrap();
        assert_eq!(at_limit.pages().len(), 1);

        let below = import_bytes(xml.as_bytes(), limits(length - 1, 1 << 20, 1 << 20, 32)).unwrap();
        assert_eq!(
            stream_error(&below),
            &ExportError::LimitExceeded {
                limit: LimitKind::Input,
                max: length - 1
            }
        );
    }

    #[test]
    fn an_input_limit_smaller_than_the_header_fails_when_opening() {
        let error =
            ExportReader::open(export_of(&[]).as_bytes(), limits(10, 100, 100, 32)).unwrap_err();
        assert_eq!(
            error,
            ExportError::LimitExceeded {
                limit: LimitKind::Input,
                max: 10
            }
        );
    }

    /// The bytes of XML from after `<page>` up to and including `</page>`.
    fn page_body_len(page: &PageFixture) -> u64 {
        let xml = page.xml();
        (xml.len() - "<page>".len()) as u64
    }

    #[test]
    fn a_page_exactly_at_the_page_limit_is_accepted_and_one_byte_more_is_not() {
        let page = PageFixture::new(1, "A");
        let size = page_body_len(&page);
        let xml = export_of(std::slice::from_ref(&page));

        let at_limit = import_bytes(xml.as_bytes(), limits(1 << 20, size, 1 << 20, 32)).unwrap();
        assert_eq!(at_limit.pages().len(), 1);

        let below = import_bytes(xml.as_bytes(), limits(1 << 20, size - 1, 1 << 20, 32)).unwrap();
        assert_eq!(
            stream_error(&below),
            &ExportError::LimitExceeded {
                limit: LimitKind::Page,
                max: size - 1
            }
        );
    }

    #[test]
    fn the_page_limit_applies_to_each_page_separately() {
        let pages = [
            PageFixture::new(1, "A"),
            PageFixture::new(2, "B"),
            PageFixture::new(3, "C"),
        ];
        let size = page_body_len(&pages[0]);
        let xml = export_of(&pages);
        let imported = import_bytes(xml.as_bytes(), limits(1 << 20, size, 1 << 20, 32)).unwrap();
        assert_eq!(imported.pages().len(), 3);
    }

    /// A reader that reports how much of the input the importer consumed.
    struct CountingInput<'a> {
        data: &'a [u8],
        position: Rc<Cell<usize>>,
    }

    impl Read for CountingInput<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let count = available.len().min(buffer.len());
            buffer[..count].copy_from_slice(&available[..count]);
            self.consume(count);
            Ok(count)
        }
    }

    impl BufRead for CountingInput<'_> {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            Ok(&self.data[self.position.get()..])
        }

        fn consume(&mut self, amount: usize) {
            self.position.set(self.position.get() + amount);
        }
    }

    #[test]
    fn an_oversized_page_is_not_read_beyond_its_limit() {
        let huge = PageFixture::new(1, "A").with_text(&"x".repeat(2 << 20));
        let xml = export_of(&[huge]);
        let page_start = xml.find("<page>").unwrap() + "<page>".len();
        let page_limit = 1_000_u64;

        let position = Rc::new(Cell::new(0));
        let input = CountingInput {
            data: xml.as_bytes(),
            position: Rc::clone(&position),
        };
        let mut reader =
            ExportReader::open(input, limits(1 << 30, page_limit, 1 << 20, 32)).unwrap();

        assert!(matches!(
            reader.next(),
            Some(Err(ImportIssue::Stream(ExportError::LimitExceeded {
                limit: LimitKind::Page,
                ..
            })))
        ));
        let page_limit = usize::try_from(page_limit).unwrap();
        assert!(
            position.get() <= page_start + page_limit,
            "read {} bytes",
            position.get()
        );
    }

    #[test]
    fn text_exactly_at_the_text_limit_is_accepted_and_one_byte_more_skips_only_that_page() {
        let at_limit = PageFixture::new(1, "A").with_text(&"a".repeat(50));
        let over = PageFixture::new(2, "B").with_text(&"b".repeat(51));
        let after = PageFixture::new(3, "C");
        let imported = import_bytes(
            export_of(&[at_limit, over, after]).as_bytes(),
            limits(1 << 20, 1 << 20, 50, 32),
        )
        .unwrap();

        let ids: Vec<u64> = imported.pages().iter().map(|page| page.id.get()).collect();
        assert_eq!(ids, [1, 3]);
        let ImportIssue::Page(issue) = imported.only_issue() else {
            panic!("expected a page issue");
        };
        assert_eq!(issue.kind, PageIssueKind::TextOverLimit);
    }

    #[test]
    fn the_text_limit_counts_unescaped_bytes() {
        // Ten bytes of text written as fifty bytes of XML.
        let escaped = "&amp;".repeat(10);
        let imported = import_bytes(
            export_of(&[PageFixture::new(1, "A").with_text(&escaped)]).as_bytes(),
            limits(1 << 20, 1 << 20, 10, 32),
        )
        .unwrap();
        assert_eq!(imported.pages()[0].text, "&".repeat(10));
    }

    #[test]
    fn nesting_exactly_at_the_depth_limit_is_accepted_and_one_level_more_is_not() {
        // mediawiki > page > revision > contributor > username is five levels deep.
        let xml = export_of(&[PageFixture::new(1, "A")]);
        let at_limit = import_bytes(xml.as_bytes(), limits(1 << 20, 1 << 20, 1 << 20, 5)).unwrap();
        assert_eq!(at_limit.pages().len(), 1);

        let imported = import_bytes(xml.as_bytes(), limits(1 << 20, 1 << 20, 1 << 20, 4)).unwrap();
        assert_eq!(
            stream_error(&imported),
            &ExportError::LimitExceeded {
                limit: LimitKind::Depth,
                max: 4
            }
        );
    }

    #[test]
    fn deeply_nested_unknown_elements_hit_the_depth_limit_without_recursion() {
        let depth = 100_000;
        let nested = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        let page = format!("<page><title>A</title>{nested}</page>");
        let imported = import(&wrap(SITEINFO, &page));
        assert!(matches!(
            stream_error(&imported),
            ExportError::LimitExceeded {
                limit: LimitKind::Depth,
                ..
            }
        ));
    }

    // --- Diagnostics never carry input text ---

    #[test]
    fn errors_for_hostile_input_contain_none_of_the_input() {
        let marker = "SECRETMARKER";
        let doctype_xml = format!(
            "<!DOCTYPE {marker} [<!ENTITY {marker} \"x\">]>{}",
            export_of(&[])
        );
        let cases = [
            // Header problems.
            doctype_xml,
            format!("<{marker}/>"),
            format!("<mediawiki xmlns=\"{NAMESPACE}\" version=\"{marker}\"/>"),
            format!(
                "<?xml version=\"1.0\" encoding=\"{marker}\"?>{}",
                export_of(&[])
            ),
            wrap(
                &format!("<siteinfo><sitename>{marker}</sitename><base>{marker}</base></siteinfo>"),
                "",
            ),
            // Stream problems.
            export_of(&[PageFixture::new(1, marker)]).replace("</title>", &format!("</{marker}>")),
            export_of(&[PageFixture::new(1, "A").with_text(&format!("&{marker};"))]),
            export_of(&[PageFixture::new(1, "A")])
                .replace("<id>1</id>", &format!("<id {marker}>1</id>")),
            export_of(&[PageFixture::new(1, "A")]).replace("<page>", &format!("<{marker}:page>")),
            wrap(SITEINFO, &format!("{marker} text")),
        ];
        for xml in &cases {
            let Err(open_failure) = ExportReader::open(xml.as_bytes(), ImportLimits::default())
            else {
                let imported = import_bytes(xml.as_bytes(), ImportLimits::default()).unwrap();
                for item in &imported.items {
                    assert_no_marker(item.as_ref().err(), marker);
                }
                assert_no_marker(imported.summary.stream_error.as_ref(), marker);
                continue;
            };
            assert_no_marker(Some(&open_failure), marker);
        }
    }

    fn assert_no_marker(error: Option<&(impl fmt::Display + fmt::Debug)>, marker: &str) {
        if let Some(error) = error {
            assert!(!error.to_string().contains(marker), "display leaks input");
            assert!(!format!("{error:?}").contains(marker), "debug leaks input");
        }
    }

    #[test]
    fn page_issues_for_hostile_pages_contain_none_of_the_input() {
        let marker = "SECRETMARKER";
        let mut page = PageFixture::new(1, marker);
        page.timestamp = Some(marker.to_owned());
        let imported = import(&export_of(&[page]));
        assert_no_marker(imported.issues().first().copied(), marker);
    }

    #[test]
    fn a_failing_source_reports_only_the_kind_of_failure() {
        #[derive(Debug)]
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "/home/secret/export.xml",
                ))
            }
        }
        impl BufRead for Failing {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "/home/secret/export.xml",
                ))
            }
            fn consume(&mut self, _amount: usize) {}
        }

        let error = ExportReader::open(Failing, ImportLimits::default()).unwrap_err();
        assert_eq!(
            error,
            ExportError::Read {
                kind: io::ErrorKind::PermissionDenied
            }
        );
        assert_no_marker(Some(&error), "secret");
    }
}
