//! The Wiki operations behind the command line: import an export into a
//! database, search it, and show a page.
//!
//! Each operation takes a plain request and returns a typed report or a
//! [`CommandError`]; nothing here parses arguments, prints or exits, so the
//! operations can be tested without a process and reused by other adapters.
//! Everything is local and offline.
//!
//! A database may hold pages from several sources. Reading operations use the
//! only source it holds, or the one named by [`Corpus::site`]; they never mix
//! sources.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::{
    ExportError, ExportReader, ExportSummary, ImportIssue, ImportLimits, ImportSummary, PageTitle,
    PrepareLimits, Resolution, SearchHit, SearchQuery, SourceId, Store, StoreError,
};

/// Why an operation failed. Messages are fixed text: they carry no path, title
/// or page text.
#[derive(Debug, Error)]
pub enum CommandError {
    /// The export file could not be opened.
    #[error("the export file could not be opened")]
    ExportFile(#[source] io::Error),
    /// The export is not a supported export, or ended with an error. Nothing
    /// from it was stored.
    #[error("the export could not be read: {0}")]
    Export(#[from] ExportError),
    /// The database failed or holds a conflicting page. Nothing from the
    /// import was stored.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The database file does not exist. Reading operations never create it.
    #[error("the database file does not exist")]
    DatabaseMissing,
    /// The database has no imported pages.
    #[error("the database holds no imported pages")]
    NoSource,
    /// The database holds several sources and none was chosen.
    #[error("the database holds {count} sources; choose one by its site name")]
    AmbiguousSource {
        /// How many sources the database holds.
        count: usize,
    },
    /// No source in the database has the chosen site name.
    #[error("the database holds no source with that site name")]
    UnknownSite,
}

/// A database to read, and which of its sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    /// The database file.
    pub database: PathBuf,
    /// The site name of the source to read. Optional when the database holds
    /// only one source.
    pub site: Option<String>,
}

/// Imports one export file into a database, creating the database if needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    /// The `Special:Export` XML file.
    pub export: PathBuf,
    /// The database file.
    pub database: PathBuf,
}

/// What an import read and stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// The wiki the export came from.
    pub source: SourceId,
    /// What the database did with the pages.
    pub stored: ImportSummary,
    /// What the export contained, including pages skipped as invalid.
    pub export: ExportSummary,
}

/// A keyword search of one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// What to search.
    pub corpus: Corpus,
    /// The validated words and limit.
    pub query: SearchQuery,
}

/// The hits of a search, best first.
#[derive(Debug)]
pub struct SearchReport {
    /// The source that was searched.
    pub source: SourceId,
    /// The matching chunks.
    pub hits: Vec<SearchHit>,
}

/// A page to show, following redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    /// What to read.
    pub corpus: Corpus,
    /// The title to look up, in any spelling the store folds together.
    pub title: String,
}

/// Where a title led.
#[derive(Debug)]
pub struct PageReport {
    /// The source that was read.
    pub source: SourceId,
    /// What the title resolved to.
    pub resolution: Resolution,
    /// The titles of the redirects followed, in order, so a reader can see how
    /// the page was reached.
    pub redirects: Vec<PageTitle>,
}

/// Imports an export in one transaction.
///
/// A stream error, a stale or conflicting revision or a database failure
/// stores nothing. Pages the importer skips as invalid are counted in the
/// report.
///
/// # Errors
///
/// See [`CommandError`].
pub fn import(request: &ImportRequest) -> Result<ImportReport, CommandError> {
    let file = File::open(&request.export).map_err(CommandError::ExportFile)?;
    let mut reader = ExportReader::open(BufReader::new(file), ImportLimits::default())?;
    let source = reader.source().clone();
    let mut store = Store::open(&request.database)?;
    let mut session = store.begin_import(&source, PrepareLimits::default())?;
    for item in reader.by_ref() {
        match item {
            Ok(page) => {
                session.add(&page)?;
            }
            // Counted in the summary; the rest of the file is still usable.
            Err(ImportIssue::Page(_)) => {}
            Err(ImportIssue::Stream(error)) => return Err(error.into()),
        }
    }
    let stored = session.commit()?;
    Ok(ImportReport {
        source,
        stored,
        export: reader.summary().clone(),
    })
}

/// Searches one source.
///
/// # Errors
///
/// See [`CommandError`]. A search with no matches is an empty report.
pub fn search(request: &SearchRequest) -> Result<SearchReport, CommandError> {
    let (store, source) = open_corpus(&request.corpus)?;
    let hits = store.search(&source, &request.query)?;
    Ok(SearchReport { source, hits })
}

/// Looks up a page by title, following redirects.
///
/// # Errors
///
/// See [`CommandError`]. A title that is missing, loops or is ambiguous is a
/// [`Resolution`] in the report, not an error.
pub fn page(request: &PageRequest) -> Result<PageReport, CommandError> {
    let (store, source) = open_corpus(&request.corpus)?;
    let resolution = store.resolve(&source, &request.title)?;
    let redirects = redirect_titles(&store, &source, &resolution)?;
    Ok(PageReport {
        source,
        resolution,
        redirects,
    })
}

fn redirect_titles(
    store: &Store,
    source: &SourceId,
    resolution: &Resolution,
) -> Result<Vec<PageTitle>, StoreError> {
    let followed = match resolution {
        Resolution::Found { redirects, .. }
        | Resolution::MissingTarget { redirects, .. }
        | Resolution::Loop { redirects }
        | Resolution::TooManyRedirects { redirects }
        | Resolution::Ambiguous { redirects, .. } => redirects.as_slice(),
        Resolution::Absent => &[],
    };
    followed
        .iter()
        .filter_map(|id| store.page(source, *id).transpose())
        .map(|stored| stored.map(|page| page.title))
        .collect()
}

fn open_corpus(corpus: &Corpus) -> Result<(Store, SourceId), CommandError> {
    if !Path::new(&corpus.database).is_file() {
        return Err(CommandError::DatabaseMissing);
    }
    let store = Store::open(&corpus.database)?;
    let source = choose_source(store.sources()?, corpus.site.as_deref())?;
    Ok((store, source))
}

/// The one source a reading operation uses. Pure.
fn choose_source(mut sources: Vec<SourceId>, site: Option<&str>) -> Result<SourceId, CommandError> {
    if let Some(site) = site {
        sources.retain(|source| source.site_name() == site);
        return match sources.len() {
            0 => Err(CommandError::UnknownSite),
            1 => Ok(sources.remove(0)),
            count => Err(CommandError::AmbiguousSource { count }),
        };
    }
    match sources.len() {
        0 => Err(CommandError::NoSource),
        1 => Ok(sources.remove(0)),
        count => Err(CommandError::AmbiguousSource { count }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::wiki::{PageId, StoreError};

    const NAMESPACE: &str = "http://www.mediawiki.org/xml/export-0.11/";

    /// One synthetic page: id, title, revision, redirect target and text.
    struct Page(u64, &'static str, u64, Option<&'static str>, &'static str);

    fn export_xml(site: &str, pages: &[Page]) -> String {
        use std::fmt::Write as _;

        let mut body = String::new();
        for Page(id, title, revision, redirect, text) in pages {
            let redirect = redirect.map_or_else(String::new, |target| {
                format!("<redirect title=\"{target}\" />")
            });
            let _ = write!(
                body,
                "<page><title>{title}</title><ns>0</ns><id>{id}</id>{redirect}<revision>\
                     <id>{revision}</id><timestamp>2026-03-04T05:06:07Z</timestamp>\
                     <model>wikitext</model><format>text/x-wiki</format>\
                     <text bytes=\"{}\" xml:space=\"preserve\">{text}</text></revision></page>",
                text.len()
            );
        }
        format!(
            "<mediawiki xmlns=\"{NAMESPACE}\" version=\"0.11\" xml:lang=\"en\">\
             <siteinfo><sitename>{site}</sitename><dbname>db</dbname>\
             <base>https://wiki.example/w/Main_Page</base><generator>MediaWiki 1.45.3</generator>\
             <case>first-letter</case><namespaces><namespace key=\"0\" case=\"first-letter\" />\
             </namespaces></siteinfo>{body}</mediawiki>"
        )
    }

    struct Workspace {
        directory: TempDir,
    }

    impl Workspace {
        fn new() -> Self {
            Self {
                directory: TempDir::new().unwrap(),
            }
        }

        fn database(&self) -> PathBuf {
            self.directory.path().join("wiki.sqlite")
        }

        fn export(&self, name: &str, xml: &str) -> PathBuf {
            let path = self.directory.path().join(name);
            fs::write(&path, xml).unwrap();
            path
        }

        fn import(&self, name: &str, xml: &str) -> Result<ImportReport, CommandError> {
            import(&ImportRequest {
                export: self.export(name, xml),
                database: self.database(),
            })
        }

        fn corpus(&self, site: Option<&str>) -> Corpus {
            Corpus {
                database: self.database(),
                site: site.map(str::to_owned),
            }
        }

        fn search(&self, query: SearchQuery) -> Result<SearchReport, CommandError> {
            search(&SearchRequest {
                corpus: self.corpus(None),
                query,
            })
        }

        fn page(&self, title: &str) -> Result<PageReport, CommandError> {
            page(&PageRequest {
                corpus: self.corpus(None),
                title: title.to_owned(),
            })
        }
    }

    fn words(text: &str) -> SearchQuery {
        SearchQuery::new(text, 10).unwrap()
    }

    fn two_pages() -> String {
        export_xml(
            "Example Wiki",
            &[
                Page(1, "Axe", 10, None, "Chops trees."),
                Page(2, "Hatchet", 20, Some("Axe"), "#REDIRECT [[Axe]]"),
            ],
        )
    }

    #[test]
    fn an_import_reports_what_was_stored_and_a_repeat_changes_nothing() {
        let workspace = Workspace::new();
        let first = workspace.import("a.xml", &two_pages()).unwrap();
        assert_eq!(first.source.site_name(), "Example Wiki");
        assert_eq!(first.stored.added, 2);
        assert_eq!(first.export.pages_imported, 2);
        assert_eq!(first.export.redirects_imported, 1);
        let again = workspace.import("a.xml", &two_pages()).unwrap();
        assert_eq!(again.stored.added, 0);
        assert_eq!(again.stored.unchanged, 2);
    }

    #[test]
    fn an_invalid_page_is_skipped_and_counted_without_stopping_the_import() {
        let workspace = Workspace::new();
        let xml = two_pages().replace("<id>2</id>", "");
        let report = workspace.import("a.xml", &xml).unwrap();
        assert_eq!(report.stored.added, 1);
        assert_eq!(report.export.total_pages_skipped(), 1);
    }

    #[test]
    fn a_missing_export_file_is_refused_before_a_database_is_created() {
        let workspace = Workspace::new();
        let result = import(&ImportRequest {
            export: workspace.directory.path().join("absent.xml"),
            database: workspace.database(),
        });
        assert!(matches!(result, Err(CommandError::ExportFile(_))));
        assert!(!workspace.database().exists());
    }

    #[test]
    fn an_unreadable_export_stores_nothing_and_leaves_the_database_unwritten() {
        let workspace = Workspace::new();
        let result = workspace.import("bad.xml", "this is not an export");
        assert!(matches!(result, Err(CommandError::Export(_))));
        assert!(!workspace.database().exists());
    }

    #[test]
    fn a_truncated_export_stores_nothing_from_the_file() {
        let workspace = Workspace::new();
        let xml = two_pages();
        let cut = &xml[..xml.len() - 40];
        assert!(matches!(
            workspace.import("cut.xml", cut),
            Err(CommandError::Export(_))
        ));
        assert!(matches!(
            workspace.search(words("trees")),
            Err(CommandError::NoSource)
        ));
    }

    #[test]
    fn a_stale_revision_fails_the_whole_file_and_names_no_title() {
        let workspace = Workspace::new();
        workspace.import("a.xml", &two_pages()).unwrap();
        let newer = export_xml(
            "Example Wiki",
            &[
                Page(3, "Pick", 30, None, "Mines rocks."),
                Page(1, "Axe", 5, None, "Older text."),
            ],
        );
        let error = workspace.import("b.xml", &newer).unwrap_err();
        assert!(matches!(
            error,
            CommandError::Store(StoreError::StaleRevision { page: 1, .. })
        ));
        let shown = error.to_string();
        assert!(
            !shown.contains("Axe") && !shown.contains("Older"),
            "{shown}"
        );
        // Page 3 came first in the file but was rolled back with it.
        assert_eq!(workspace.search(words("rocks")).unwrap().hits.len(), 0);
    }

    #[test]
    fn a_search_returns_hits_from_the_imported_source() {
        let workspace = Workspace::new();
        workspace.import("a.xml", &two_pages()).unwrap();
        let report = workspace.search(words("trees")).unwrap();
        assert_eq!(report.source.site_name(), "Example Wiki");
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].title.as_str(), "Axe");
    }

    #[test]
    fn reading_a_database_that_does_not_exist_does_not_create_it() {
        let workspace = Workspace::new();
        assert!(matches!(
            workspace.search(words("trees")),
            Err(CommandError::DatabaseMissing)
        ));
        assert!(matches!(
            workspace.page("Axe"),
            Err(CommandError::DatabaseMissing)
        ));
        assert!(!workspace.database().exists());
    }

    #[test]
    fn a_page_follows_a_redirect_and_names_the_redirects_it_followed() {
        let workspace = Workspace::new();
        workspace.import("a.xml", &two_pages()).unwrap();
        let report = workspace.page("hatchet").unwrap();
        let Resolution::Found { page, .. } = &report.resolution else {
            panic!("expected a page");
        };
        assert_eq!(page.id, PageId::new(1).unwrap());
        let titles: Vec<&str> = report.redirects.iter().map(PageTitle::as_str).collect();
        assert_eq!(titles, ["Hatchet"]);
    }

    #[test]
    fn a_redirect_to_a_page_not_imported_reports_the_missing_target() {
        let workspace = Workspace::new();
        let xml = export_xml(
            "Example Wiki",
            &[Page(
                2,
                "Hatchet",
                20,
                Some("Gone page"),
                "#REDIRECT [[Gone page]]",
            )],
        );
        workspace.import("a.xml", &xml).unwrap();
        let report = workspace.page("Hatchet").unwrap();
        assert!(matches!(
            report.resolution,
            Resolution::MissingTarget { target, .. } if target.as_str() == "Gone page"
        ));
    }

    #[test]
    fn several_sources_need_a_site_name_to_choose_between_them() {
        let workspace = Workspace::new();
        workspace.import("a.xml", &two_pages()).unwrap();
        workspace
            .import(
                "b.xml",
                &export_xml("Other Wiki", &[Page(1, "Axe", 10, None, "Elsewhere.")]),
            )
            .unwrap();
        assert!(matches!(
            workspace.search(words("trees")),
            Err(CommandError::AmbiguousSource { count: 2 })
        ));
        let chosen = search(&SearchRequest {
            corpus: workspace.corpus(Some("Other Wiki")),
            query: SearchQuery::new("elsewhere", 10).unwrap(),
        })
        .unwrap();
        assert_eq!(chosen.hits.len(), 1);
        let unknown = search(&SearchRequest {
            corpus: workspace.corpus(Some("Nope")),
            query: SearchQuery::new("trees", 10).unwrap(),
        });
        assert!(matches!(unknown, Err(CommandError::UnknownSite)));
    }

    #[test]
    fn choosing_a_source_is_pure() {
        let source = |name: &str| {
            SourceId::new(name.to_owned(), None, "https://w.example/".to_owned()).unwrap()
        };
        assert!(matches!(
            choose_source(Vec::new(), None),
            Err(CommandError::NoSource)
        ));
        assert!(matches!(
            choose_source(Vec::new(), Some("A")),
            Err(CommandError::UnknownSite)
        ));
        assert_eq!(
            choose_source(vec![source("A")], None).unwrap().site_name(),
            "A"
        );
        assert_eq!(
            choose_source(vec![source("A"), source("B")], Some("B"))
                .unwrap()
                .site_name(),
            "B"
        );
    }
}
