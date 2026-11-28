//! Runs a dataset's queries against an imported Wiki corpus and scores them.
//!
//! The runner is offline and read-only. It observes which dataset pages and
//! sections the corpus holds, searches each scorable query once with the
//! keyword search, maps the hits onto the dataset's canonical titles and hands
//! both to the pure scorer. Aggregation and reporting live in `summary`.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use super::scorer::{self, CorpusTitle, Hit, QueryScore, ScoringTier, SectionContent};
use super::{Dataset, Query};
use crate::wiki::{
    ChunkKind, InvalidSearch, MAX_RESULTS, PageId, Resolution, SearchHit, SearchQuery, SourceId,
    Store, StoreError, StoredChunk,
};

/// Rank cutoffs reported next to the run's own `k`, where they fit within it.
const STANDARD_CUTOFFS: [u32; 4] = [1, 3, 5, 10];

const SECTION_SEPARATOR: &str = " > ";

/// Whether the corpus under test is real Wiki content or a synthetic fixture.
///
/// A synthetic corpus needs its own ground truth; the committed dataset's
/// references describe the real Wiki.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusKind {
    /// Pages exported from the real Wiki by a person.
    Real,
    /// Invented pages written for a test.
    Synthetic,
}

/// How a run searches and scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunConfig {
    k: NonZeroU32,
    corpus_kind: CorpusKind,
}

impl RunConfig {
    /// A run that keeps the best `k` hits per query.
    ///
    /// # Errors
    ///
    /// Fails if `k` is more than the search returns, [`MAX_RESULTS`].
    pub fn new(k: NonZeroU32, corpus_kind: CorpusKind) -> Result<Self, InvalidSearch> {
        if usize::try_from(k.get()).map_or(true, |limit| limit > MAX_RESULTS) {
            return Err(InvalidSearch::LimitTooLarge);
        }
        Ok(Self { k, corpus_kind })
    }

    /// The number of hits kept per query.
    #[must_use]
    pub const fn k(self) -> NonZeroU32 {
        self.k
    }

    /// What kind of corpus the run is over.
    #[must_use]
    pub const fn corpus_kind(self) -> CorpusKind {
        self.corpus_kind
    }

    /// The cutoffs scores are reported at: the standard ones that fit within
    /// `k`, and `k` itself.
    fn cutoffs(self) -> Vec<NonZeroU32> {
        let mut cutoffs: Vec<_> = STANDARD_CUTOFFS
            .into_iter()
            .filter(|cutoff| *cutoff < self.k.get())
            .filter_map(NonZeroU32::new)
            .collect();
        cutoffs.push(self.k);
        cutoffs
    }
}

/// What the corpus says about a requested title, against the canonical page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleOutcome {
    /// The title leads to the canonical page.
    Canonical,
    /// The title leads to a different page, or the canonical page is absent.
    Elsewhere,
    /// The title is a redirect whose target was not imported.
    TargetMissing,
    /// The title was not imported.
    Absent,
    /// The redirects could not be followed to a single page.
    Unknown,
}

/// One dataset page as the corpus holds it.
struct ObservedPage {
    id: PageId,
    revision: u64,
    sections: BTreeMap<String, SectionContent>,
}

/// What the corpus holds for one dataset title.
enum Observation {
    Present(ObservedPage),
    Missing,
    RedirectTargetMissing(String),
}

/// The corpus as seen from the dataset: for each judged title, whether it and
/// its sections were imported, and how its alternative titles resolve.
///
/// A title whose redirects cannot be followed to one page has no observation,
/// which the scorer reports as unknown coverage.
pub struct CorpusObservations {
    titles: BTreeMap<String, Observation>,
    redirects: BTreeMap<String, TitleOutcome>,
}

impl fmt::Debug for CorpusObservations {
    /// Shows counts only: titles are export content.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CorpusObservations")
            .field("titles", &self.titles.len())
            .finish_non_exhaustive()
    }
}

impl CorpusObservations {
    /// Reads from `store` what the corpus holds for every page `dataset`
    /// judges and every title it records as resolving to one.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn observe(
        store: &Store,
        source: &SourceId,
        dataset: &Dataset,
    ) -> Result<Self, StoreError> {
        let mut titles = BTreeMap::new();
        for query in dataset.queries() {
            for judgement in &query.judgements {
                if !titles.contains_key(&judgement.page)
                    && let Some(observation) = observe_title(store, source, &judgement.page)?
                {
                    titles.insert(judgement.page.clone(), observation);
                }
            }
        }
        let mut redirects = BTreeMap::new();
        for page in dataset.pages() {
            let canonical = titles
                .get(&page.title)
                .and_then(|observation| match observation {
                    Observation::Present(page) => Some(page.id),
                    _ => None,
                });
            for requested in &page.requested_as {
                let outcome = title_outcome(store.resolve(source, requested)?, canonical);
                redirects.insert(requested.clone(), outcome);
            }
        }
        Ok(Self { titles, redirects })
    }

    /// How an alternative title recorded by the dataset resolves.
    #[must_use]
    pub fn redirect_outcome(&self, requested: &str) -> Option<TitleOutcome> {
        self.redirects.get(requested).copied()
    }

    /// How a judged page's own title resolves: to itself, to nothing, or to a
    /// redirect whose target was not imported. A title the corpus could not
    /// settle is unknown.
    #[must_use]
    pub fn title_outcome(&self, title: &str) -> TitleOutcome {
        match self.titles.get(title) {
            Some(Observation::Present(_)) => TitleOutcome::Canonical,
            Some(Observation::Missing) => TitleOutcome::Absent,
            Some(Observation::RedirectTargetMissing(_)) => TitleOutcome::TargetMissing,
            None => TitleOutcome::Unknown,
        }
    }

    /// The title the query's judgements use for the imported page with this
    /// id. A judged title may itself be a redirect to the page, in which case
    /// hits on the page count under that title.
    fn judged_title<'q>(&self, id: PageId, query: &'q Query) -> Option<&'q str> {
        query
            .judgements
            .iter()
            .find_map(|judgement| match self.titles.get(&judgement.page)? {
                Observation::Present(page) if page.id == id => Some(judgement.page.as_str()),
                _ => None,
            })
    }

    /// Hands the scorer's view of the corpus to `use_corpus`. The view borrows
    /// per-page section maps, so it only lives for the call.
    fn with_scoring_corpus<R>(
        &self,
        use_corpus: impl FnOnce(&BTreeMap<&str, CorpusTitle<'_>>) -> R,
    ) -> R {
        let section_maps: BTreeMap<&str, BTreeMap<&str, SectionContent>> = self
            .titles
            .iter()
            .filter_map(|(title, observation)| match observation {
                Observation::Present(page) => Some((
                    title.as_str(),
                    page.sections
                        .iter()
                        .map(|(path, content)| (path.as_str(), *content))
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        let corpus = self
            .titles
            .iter()
            .map(|(title, observation)| {
                let view = match observation {
                    Observation::Present(page) => CorpusTitle::Present {
                        revision: Some(page.revision),
                        sections: &section_maps[title.as_str()],
                    },
                    Observation::Missing => CorpusTitle::Missing,
                    Observation::RedirectTargetMissing(target) => {
                        CorpusTitle::RedirectTargetMissing(target)
                    }
                };
                (title.as_str(), view)
            })
            .collect();
        use_corpus(&corpus)
    }

    /// The redirect target a title's redirect names, when it was not imported.
    fn missing_redirect_target(&self, title: &str) -> Option<&str> {
        match self.titles.get(title) {
            Some(Observation::RedirectTargetMissing(target)) => Some(target),
            _ => None,
        }
    }
}

fn observe_title(
    store: &Store,
    source: &SourceId,
    title: &str,
) -> Result<Option<Observation>, StoreError> {
    Ok(match store.resolve(source, title)? {
        Resolution::Found { page, .. } => {
            let chunks = store.chunks(source, page.id)?;
            Some(Observation::Present(ObservedPage {
                id: page.id,
                revision: page.revision.get(),
                sections: section_contents(&chunks),
            }))
        }
        Resolution::Absent => Some(Observation::Missing),
        Resolution::MissingTarget { target, .. } => Some(Observation::RedirectTargetMissing(
            target.as_str().to_owned(),
        )),
        Resolution::Loop { .. }
        | Resolution::TooManyRedirects { .. }
        | Resolution::Ambiguous { .. } => None,
    })
}

fn title_outcome(resolution: Resolution, canonical: Option<PageId>) -> TitleOutcome {
    match resolution {
        Resolution::Found { page, .. } if Some(page.id) == canonical => TitleOutcome::Canonical,
        Resolution::Found { .. } => TitleOutcome::Elsewhere,
        Resolution::MissingTarget { .. } => TitleOutcome::TargetMissing,
        Resolution::Absent => TitleOutcome::Absent,
        Resolution::Loop { .. }
        | Resolution::TooManyRedirects { .. }
        | Resolution::Ambiguous { .. } => TitleOutcome::Unknown,
    }
}

/// The content status of every section path that has chunks, and of every
/// ancestor of one: a parent whose text lives in its subsections is still a
/// section a hit can fall under. A section is template-only unless some chunk
/// at or beneath it has prose.
fn section_contents(chunks: &[StoredChunk]) -> BTreeMap<String, SectionContent> {
    let mut contents: BTreeMap<String, SectionContent> = BTreeMap::new();
    for chunk in chunks {
        let content = match chunk.kind {
            ChunkKind::Prose => SectionContent::Usable,
            ChunkKind::TemplateOnly => SectionContent::TemplateOnly,
        };
        for depth in 1..=chunk.section_path.len() {
            let path = chunk.section_path[..depth].join(SECTION_SEPARATOR);
            let entry = contents.entry(path).or_insert(content);
            if content == SectionContent::Usable {
                *entry = SectionContent::Usable;
            }
        }
    }
    contents
}

/// One result in rank order, mapped onto the dataset's titles.
#[derive(Clone, PartialEq, Eq)]
pub struct RankedHit {
    page: String,
    section: Option<String>,
    text_chars: usize,
}

impl fmt::Debug for RankedHit {
    /// Shows sizes only: titles and text are export content.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RankedHit")
            .field("has_section", &self.section.is_some())
            .field("text_chars", &self.text_chars)
            .finish_non_exhaustive()
    }
}

impl RankedHit {
    /// The page title, in the dataset's spelling when the page is judged.
    #[must_use]
    pub fn page(&self) -> &str {
        &self.page
    }

    /// The section path, or `None` for text before the first heading.
    #[must_use]
    pub fn section(&self) -> Option<&str> {
        self.section.as_deref()
    }

    /// The characters of text this result returns to a caller.
    #[must_use]
    pub const fn text_chars(&self) -> usize {
        self.text_chars
    }

    fn as_scorer_hit(&self) -> Hit<'_> {
        Hit {
            page: &self.page,
            section: self.section.as_deref(),
        }
    }
}

/// Whether and how a query was searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    /// The search ran; it may have found nothing.
    Searched,
    /// The query text cannot be searched, so it found nothing.
    Rejected(InvalidSearch),
    /// The query is excluded from scores, so it was not searched.
    NotRun,
}

/// The score of one query at one rank cutoff.
#[derive(Debug, Clone, PartialEq)]
pub struct CutoffScore {
    /// The rank cutoff.
    pub k: NonZeroU32,
    /// The scorer's result at that cutoff.
    pub score: QueryScore,
}

/// One query's results, scores and costs.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryRun {
    /// The dataset's query identifier.
    pub id: String,
    /// Whether the query was searched.
    pub search: SearchOutcome,
    /// The ranked results, at most `k`.
    pub hits: Vec<RankedHit>,
    /// Wall-clock time of the search call alone.
    pub latency: Duration,
    /// The query's scores at each reported cutoff, smallest first. Empty
    /// when the query is unknown to the scorer.
    pub scores: Vec<CutoffScore>,
}

impl QueryRun {
    /// The characters of text returned by all of the query's hits.
    #[must_use]
    pub fn response_chars(&self) -> usize {
        self.hits.iter().map(RankedHit::text_chars).sum()
    }
}

/// Everything a run produced: the corpus observations and each query's run,
/// in dataset order.
#[derive(Debug)]
pub struct EvaluationRun {
    config: RunConfig,
    observations: CorpusObservations,
    queries: Vec<QueryRun>,
}

impl EvaluationRun {
    /// The configuration the run used.
    #[must_use]
    pub const fn config(&self) -> RunConfig {
        self.config
    }

    /// What the corpus holds for the dataset's pages.
    #[must_use]
    pub const fn observations(&self) -> &CorpusObservations {
        &self.observations
    }

    /// Each query's run, in dataset order.
    #[must_use]
    pub fn queries(&self) -> &[QueryRun] {
        &self.queries
    }

    /// The titles to request, one per page, from the missing pages of every
    /// scorable query's essential judgements: a missing page's own title, and
    /// for a redirect whose target was not imported, that target.
    #[must_use]
    pub fn missing_pages(&self, dataset: &Dataset) -> Vec<String> {
        let mut titles = std::collections::BTreeSet::new();
        for (query, run) in dataset.queries().iter().zip(&self.queries) {
            let Some(score) = run.scores.last().map(|entry| &entry.score) else {
                continue;
            };
            if score.tier == ScoringTier::Excluded {
                continue;
            }
            for (judgement, assessment) in query.judgements.iter().zip(&score.assessments) {
                match assessment.availability {
                    scorer::Availability::PageMissing if judgement.grade == 2 => {
                        titles.insert(judgement.page.clone());
                    }
                    scorer::Availability::RedirectTargetMissing if judgement.grade == 2 => {
                        if let Some(target) =
                            self.observations.missing_redirect_target(&judgement.page)
                        {
                            titles.insert(target.to_owned());
                        }
                    }
                    _ => {}
                }
            }
        }
        titles.into_iter().collect()
    }
}

/// Runs every scorable query of `dataset` against `source` and scores it.
///
/// # Errors
///
/// Fails on a database error. A query that cannot be searched is recorded in
/// its [`QueryRun`], not an error.
pub fn run(
    store: &Store,
    source: &SourceId,
    dataset: &Dataset,
    config: RunConfig,
) -> Result<EvaluationRun, StoreError> {
    let observations = CorpusObservations::observe(store, source, dataset)?;
    let mut queries = Vec::with_capacity(dataset.queries().len());
    for query in dataset.queries() {
        queries.push(run_query(
            store,
            source,
            dataset,
            &observations,
            config,
            query,
        )?);
    }
    Ok(EvaluationRun {
        config,
        observations,
        queries,
    })
}

fn run_query(
    store: &Store,
    source: &SourceId,
    dataset: &Dataset,
    observations: &CorpusObservations,
    config: RunConfig,
    query: &Query,
) -> Result<QueryRun, StoreError> {
    let (search, hits, latency) = if ScoringTier::from_query(query) == ScoringTier::Excluded {
        (SearchOutcome::NotRun, Vec::new(), Duration::ZERO)
    } else {
        search_query(store, source, observations, config, query)?
    };
    let scores = score_at_cutoffs(dataset, observations, config, query, &hits);
    Ok(QueryRun {
        id: query.id.clone(),
        search,
        hits,
        latency,
        scores,
    })
}

fn search_query(
    store: &Store,
    source: &SourceId,
    observations: &CorpusObservations,
    config: RunConfig,
    query: &Query,
) -> Result<(SearchOutcome, Vec<RankedHit>, Duration), StoreError> {
    let limit = usize::try_from(config.k.get()).map_err(|_| StoreError::IdentifierTooLarge)?;
    let search = match SearchQuery::new(&query.text, limit) {
        Ok(search) => search,
        Err(invalid) => return Ok((SearchOutcome::Rejected(invalid), Vec::new(), Duration::ZERO)),
    };
    let started = Instant::now();
    let found = store.search(source, &search)?;
    let latency = started.elapsed();
    let hits = rank_hits(observations, query, found);
    Ok((SearchOutcome::Searched, hits, latency))
}

/// Maps hits onto the titles the query's judgements use. A search only
/// returns chunks, and a redirect page has none, so every hit is on a page
/// that is not a redirect.
fn rank_hits(
    observations: &CorpusObservations,
    query: &Query,
    found: Vec<SearchHit>,
) -> Vec<RankedHit> {
    found
        .into_iter()
        .map(|hit| {
            let path = hit.chunk.section_path.join(SECTION_SEPARATOR);
            RankedHit {
                page: observations
                    .judged_title(hit.page, query)
                    .unwrap_or_else(|| hit.title.as_str())
                    .to_owned(),
                section: (!path.is_empty()).then_some(path),
                text_chars: hit.chunk.text.chars().count(),
            }
        })
        .collect()
}

fn score_at_cutoffs(
    dataset: &Dataset,
    observations: &CorpusObservations,
    config: RunConfig,
    query: &Query,
    hits: &[RankedHit],
) -> Vec<CutoffScore> {
    let scorer_hits: Vec<_> = hits.iter().map(RankedHit::as_scorer_hit).collect();
    observations.with_scoring_corpus(|corpus| {
        config
            .cutoffs()
            .into_iter()
            .filter_map(|k| {
                scorer::score_query(dataset, &query.id, corpus, &scorer_hits, k)
                    .map(|score| CutoffScore { k, score })
            })
            .collect()
    })
}

/// An invented corpus and dataset with their own ground truth, shared with the
/// summary tests. Nothing here comes from the real Wiki.
#[cfg(test)]
pub(super) mod synthetic {
    use std::fmt::Write as _;
    use std::io::Cursor;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::wiki::{ExportReader, ImportIssue, ImportLimits, PrepareLimits};

    /// One page: id, title, revision, redirect target and wikitext.
    pub(in crate::eval) struct Page(
        pub u64,
        pub &'static str,
        pub u64,
        pub Option<&'static str>,
        pub &'static str,
    );

    const WORKSHOP: &str = "== Tools ==\nHammers and saws live here.\n=== Measuring ===\n\
                            Rulers and tapes for measuring.\n== Supplies ==\nNails, screws and glue.\n\
                            == Notes ==\n{{Opening hours}}";
    const GARDEN: &str = "== Planting ==\nSeeds go in the soil in spring.\n\
                          == Watering ==\nWater the seeds early.";

    pub(in crate::eval) fn pages(workshop_revision: u64) -> Vec<Page> {
        vec![
            Page(1, "Workshop", workshop_revision, None, WORKSHOP),
            Page(2, "Garden", 3, None, GARDEN),
            Page(3, "Shed", 4, Some("Storehouse"), "#REDIRECT [[Storehouse]]"),
            Page(4, "Yard", 5, Some("Garden"), "#REDIRECT [[Garden]]"),
        ]
    }

    pub(in crate::eval) fn export_xml(pages: &[Page]) -> String {
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
            "<mediawiki xmlns=\"http://www.mediawiki.org/xml/export-0.11/\" version=\"0.11\" \
             xml:lang=\"en\"><siteinfo><sitename>Workshop Wiki</sitename>\
             <dbname>workshopwiki</dbname><base>https://workshop.example/w/Main_Page</base>\
             <generator>MediaWiki 1.45.3</generator><case>first-letter</case>\
             <namespaces><namespace key=\"0\" case=\"first-letter\" /></namespaces>\
             </siteinfo>{body}</mediawiki>"
        )
    }

    /// A database in a temporary directory, with the source it holds.
    pub(in crate::eval) struct Corpus {
        _directory: TempDir,
        pub store: Store,
        pub source: SourceId,
    }

    pub(in crate::eval) fn corpus(pages: &[Page]) -> Corpus {
        let directory = TempDir::new().unwrap();
        let mut store = Store::open(&directory.path().join("corpus.sqlite")).unwrap();
        let mut reader =
            ExportReader::open(Cursor::new(export_xml(pages)), ImportLimits::default()).unwrap();
        let source = reader.source().clone();
        let mut session = store
            .begin_import(&source, PrepareLimits::default())
            .unwrap();
        for item in reader.by_ref() {
            match item {
                Ok(page) => {
                    session.add(&page).unwrap();
                }
                Err(ImportIssue::Page(_)) => {}
                Err(ImportIssue::Stream(_)) => panic!("synthetic export is well formed"),
            }
        }
        session.commit().unwrap();
        Corpus {
            _directory: directory,
            store,
            source,
        }
    }

    fn query(
        id: &str,
        text: &str,
        relevance: &str,
        tags: &[&str],
        judgements: &serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "category": if tags.contains(&"redirect") { "exact_name" } else { "lookup" },
            "mode": "mode_independent",
            "query": text,
            "intent": "Exercise the runner",
            "references": "verified",
            "relevance": relevance,
            "tags": tags,
            "judgements": judgements,
            "notes": "Synthetic independent judgements"
        })
    }

    /// The dataset's expectations, written from the corpus above, not from
    /// anything the search returns:
    ///
    /// - `q001`: found through a subsection of the judged section.
    /// - `q002`: one target found, one page never imported.
    /// - `q003`: the judged section is only a template call.
    /// - `q004`: the judged page is a redirect to a page never imported.
    /// - `q005`: judged under a redirect title that leads to the imported page.
    /// - `q006`: excluded as ambiguous, so never searched.
    /// - `q007`: scorable, but its text has no searchable word.
    pub(in crate::eval) fn dataset(workshop_oldid: u64) -> Dataset {
        let page = |title: &str, requested_as: &[&str], oldid: Option<u64>, sections: &[&str]| {
            json!({
                "title": title,
                "requested_as": requested_as,
                "oldid": oldid,
                "last_modified": oldid.map(|_| "2026-01-01"),
                "revision_basis": if oldid.is_some() { "footer" } else { "not_observed" },
                "headings_complete": true,
                "observed_sections": sections,
                "note": null
            })
        };
        let data = json!({
            "format_version": 1,
            "dataset_id": "synthetic-workshop-run",
            "dataset_version": "1.0.0",
            "created": "2026-01-01",
            "source": {
                "site": "https://workshop.example",
                "observed_on": "2026-01-01",
                "method": "original fixture",
                "licence_note": "synthetic records"
            },
            "modes": {"mode_independent": "Independent"},
            "grades": {"1": "Supporting", "2": "Essential"},
            "pages": [
                page(
                    "Workshop",
                    &[],
                    Some(workshop_oldid),
                    &["Tools", "Tools > Measuring", "Supplies", "Notes"],
                ),
                page("Garden", &["Yard", "Plot"], None, &["Planting", "Watering"]),
                page("Yard", &[], None, &["Overview"]),
                page("Orchard", &[], None, &["Overview"]),
                page("Shed", &[], None, &["Overview"]),
            ],
            "queries": [
                query("q001", "rulers measuring", "provisional", &[], &json!([
                    {"page": "Workshop", "section": "Tools", "grade": 2},
                    {"page": "Workshop", "section": "Supplies", "grade": 1}
                ])),
                query("q002", "seeds spring", "provisional", &[], &json!([
                    {"page": "Garden", "section": "Planting", "grade": 2},
                    {"page": "Orchard", "section": null, "grade": 2}
                ])),
                query("q003", "opening hours", "provisional", &[], &json!([
                    {"page": "Workshop", "section": "Notes", "grade": 2}
                ])),
                query("q004", "shed storehouse", "provisional", &["redirect"], &json!([
                    {"page": "Shed", "section": null, "grade": 2}
                ])),
                query("q005", "water seeds early", "provisional", &["redirect"], &json!([
                    {"page": "Yard", "section": null, "grade": 2}
                ])),
                query("q006", "anything", "ambiguous", &[], &json!([
                    {"page": "Garden", "section": null, "grade": 2}
                ])),
                query("q007", "\u{1}", "provisional", &[], &json!([
                    {"page": "Garden", "section": null, "grade": 2}
                ])),
            ],
            "retired_query_ids": []
        });
        Dataset::parse(&data.to_string()).unwrap()
    }

    pub(in crate::eval) fn config(k: u32) -> RunConfig {
        RunConfig::new(NonZeroU32::new(k).unwrap(), CorpusKind::Synthetic).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::scorer::{Availability, RevisionMatch};
    use super::synthetic::{config, corpus, dataset, pages};
    use super::*;

    fn run_all(workshop_revision: u64, workshop_oldid: u64, k: u32) -> (Dataset, EvaluationRun) {
        let corpus = corpus(&pages(workshop_revision));
        let dataset = dataset(workshop_oldid);
        let run = run(&corpus.store, &corpus.source, &dataset, config(k)).unwrap();
        (dataset, run)
    }

    fn query<'a>(run: &'a EvaluationRun, id: &str) -> &'a QueryRun {
        run.queries().iter().find(|query| query.id == id).unwrap()
    }

    fn at(run: &QueryRun, k: u32) -> &QueryScore {
        &run.scores
            .iter()
            .find(|entry| entry.k.get() == k)
            .unwrap()
            .score
    }

    #[test]
    fn a_hit_in_a_subsection_satisfies_the_judged_section() {
        let (_, run) = run_all(7, 7, 10);
        let q001 = query(&run, "q001");
        assert_eq!(q001.search, SearchOutcome::Searched);
        assert_eq!(q001.hits.len(), 1);
        assert_eq!(q001.hits[0].page(), "Workshop");
        assert_eq!(q001.hits[0].section(), Some("Tools > Measuring"));
        let section = at(q001, 10).section.unwrap();
        assert_eq!(section.end_to_end.matched, 1);
        assert!((section.end_to_end.reciprocal_rank - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_page_that_was_never_imported_stays_in_the_end_to_end_denominator() {
        let (_, run) = run_all(7, 7, 10);
        let score = at(query(&run, "q002"), 10);
        assert_eq!(
            score
                .assessments
                .iter()
                .map(|assessment| assessment.availability)
                .collect::<Vec<_>>(),
            [Availability::Available, Availability::PageMissing]
        );
        let page = score.page.unwrap();
        assert_eq!(page.end_to_end.relevant, 2);
        assert_eq!(page.conditional.unwrap().relevant, 1);
        assert!((page.end_to_end.recall - 0.5).abs() < f64::EPSILON);
        assert!((page.conditional.unwrap().recall - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_template_only_section_is_found_by_search_but_not_counted_as_success() {
        let (_, run) = run_all(7, 7, 10);
        let q003 = query(&run, "q003");
        assert_eq!(q003.hits[0].section(), Some("Notes"));
        let score = at(q003, 10);
        assert_eq!(
            score.assessments[0].availability,
            Availability::TemplateOnly
        );
        let section = score.section.unwrap();
        assert_eq!(section.end_to_end.matched, 0);
        assert!(section.conditional.is_none());
    }

    #[test]
    fn a_redirect_without_its_target_is_a_coverage_gap_not_a_ranking_miss() {
        let (dataset, run) = run_all(7, 7, 10);
        let q004 = query(&run, "q004");
        assert!(
            q004.hits.is_empty(),
            "a redirect page has no chunks to find"
        );
        assert_eq!(
            at(q004, 10).assessments[0].availability,
            Availability::RedirectTargetMissing
        );
        assert!(!at(q004, 10).page.unwrap().end_to_end.hit);
        assert_eq!(run.missing_pages(&dataset), ["Orchard", "Storehouse"]);
    }

    #[test]
    fn a_hit_counts_under_the_redirect_title_the_dataset_judged() {
        let (_, run) = run_all(7, 7, 10);
        let q005 = query(&run, "q005");
        assert_eq!(q005.hits[0].page(), "Yard");
        assert_eq!(q005.hits[0].section(), Some("Watering"));
        assert!(at(q005, 10).page.unwrap().end_to_end.hit);
    }

    #[test]
    fn excluded_and_unsearchable_queries_are_recorded_not_scored_as_misses() {
        let (_, run) = run_all(7, 7, 10);
        let q006 = query(&run, "q006");
        assert_eq!(q006.search, SearchOutcome::NotRun);
        assert_eq!(at(q006, 10).tier, ScoringTier::Excluded);
        assert!(at(q006, 10).page.is_none());
        let q007 = query(&run, "q007");
        assert_eq!(q007.search, SearchOutcome::Rejected(InvalidSearch::Empty));
        assert_eq!(q007.hits, []);
        assert!(!at(q007, 10).page.unwrap().end_to_end.hit);
    }

    #[test]
    fn a_revision_that_differs_from_the_dataset_is_reported_beside_the_availability() {
        let (_, run) = run_all(8, 7, 10);
        let assessment = at(query(&run, "q001"), 10).assessments[0];
        assert_eq!(assessment.availability, Availability::Available);
        assert_eq!(
            assessment.revision,
            RevisionMatch::Mismatch {
                expected: 7,
                observed: 8
            }
        );
    }

    #[test]
    fn alternative_titles_resolve_against_the_canonical_page() {
        let (_, run) = run_all(7, 7, 10);
        let observations = run.observations();
        assert_eq!(
            observations.redirect_outcome("Yard"),
            Some(TitleOutcome::Canonical)
        );
        assert_eq!(
            observations.redirect_outcome("Plot"),
            Some(TitleOutcome::Absent)
        );
        assert_eq!(observations.redirect_outcome("Nowhere"), None);
        assert_eq!(
            observations.title_outcome("Shed"),
            TitleOutcome::TargetMissing
        );
        assert_eq!(observations.title_outcome("Orchard"), TitleOutcome::Absent);
    }

    #[test]
    fn scores_are_reported_at_the_standard_cutoffs_that_fit_within_k() {
        let ks = |k| {
            let (_, run) = run_all(7, 7, k);
            query(&run, "q001")
                .scores
                .iter()
                .map(|entry| entry.k.get())
                .collect::<Vec<_>>()
        };
        assert_eq!(ks(10), [1, 3, 5, 10]);
        assert_eq!(ks(4), [1, 3, 4]);
        assert_eq!(ks(1), [1]);
    }

    #[test]
    fn k_beyond_what_search_returns_is_refused() {
        let k = NonZeroU32::new(u32::try_from(MAX_RESULTS).unwrap() + 1).unwrap();
        assert_eq!(
            RunConfig::new(k, CorpusKind::Real).unwrap_err(),
            InvalidSearch::LimitTooLarge
        );
    }

    #[test]
    fn debug_output_shows_no_titles() {
        let (_, run) = run_all(7, 7, 10);
        let text = format!("{run:?}");
        for title in ["Workshop", "Garden", "Storehouse"] {
            assert!(!text.contains(title), "{title} leaked into Debug output");
        }
    }
}
