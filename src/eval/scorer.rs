//! Pure scoring of ranked retrieval hits and corpus evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU32;

use super::{Dataset, Judgement, Query, References, Relevance, covers};

const ESSENTIAL: u8 = 2;

/// The searchable content available for a section in the corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionContent {
    /// The section contains usable searchable text.
    Usable,
    /// The section's substance is unavailable without template expansion.
    TemplateOnly,
}

/// What the corpus reports for a dataset title.
#[derive(Clone, Copy)]
pub enum CorpusTitle<'a> {
    /// The page is present, with known section paths and their content status.
    Present {
        /// Imported revision, if recorded.
        revision: Option<u64>,
        /// Section paths present on the page.
        sections: &'a BTreeMap<&'a str, SectionContent>,
    },
    /// The title was checked and no page or redirect was recorded.
    Missing,
    /// Recorded redirect destination with no imported page.
    RedirectTargetMissing(&'a str),
}

impl fmt::Debug for CorpusTitle<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Present { revision, sections } => formatter
                .debug_struct("Present")
                .field("revision", revision)
                .field("section_count", &sections.len())
                .finish(),
            Self::Missing => formatter.write_str("Missing"),
            Self::RedirectTargetMissing(_) => formatter.write_str("RedirectTargetMissing(..)"),
        }
    }
}

/// One ranked result. Multiple chunks from the same section have separate ranks.
#[derive(Clone, Copy)]
pub struct Hit<'a> {
    /// Canonical page title after redirect resolution.
    pub page: &'a str,
    /// Section path, if this is a section result.
    pub section: Option<&'a str>,
}

impl fmt::Debug for Hit<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Hit")
            .field("has_section", &self.section.is_some())
            .finish_non_exhaustive()
    }
}

/// How an evaluated judgement relates to the corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// All evidence necessary for this judgement is available.
    Available,
    /// The checked title is absent from the corpus.
    PageMissing,
    /// The recorded redirect leads to a page absent from the corpus.
    RedirectTargetMissing,
    /// The page is present and the section path is missing.
    SectionMissing,
    /// The page is present and the section requires template expansion.
    TemplateOnly,
    /// The corpus has no observation for this title.
    Unknown,
}

impl Availability {
    const fn page_is_present(self) -> bool {
        matches!(
            self,
            Self::Available | Self::SectionMissing | Self::TemplateOnly
        )
    }
}

/// Comparison of the dataset and corpus revision identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionMatch {
    /// The two recorded revision identifiers agree.
    Matches,
    /// The two recorded revision identifiers differ.
    Mismatch {
        /// Identifier recorded by the dataset.
        expected: u64,
        /// Identifier on the imported page.
        observed: u64,
    },
    /// At least one revision identifier was unavailable.
    Unknown,
}

impl RevisionMatch {
    const fn compare(expected: Option<u64>, observed: Option<u64>) -> Self {
        match (expected, observed) {
            (Some(expected), Some(observed)) if expected == observed => Self::Matches,
            (Some(expected), Some(observed)) => Self::Mismatch { expected, observed },
            _ => Self::Unknown,
        }
    }
}

/// Availability and revision evidence for a judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assessment {
    /// Availability at the level specified by the judgement.
    pub availability: Availability,
    /// Revision agreement, independent of path availability.
    pub revision: RevisionMatch,
}

impl Assessment {
    /// Whether the page itself is present, even if a section is unavailable.
    #[must_use]
    pub const fn page_is_present(self) -> bool {
        self.availability.page_is_present()
    }
}

/// Whether a query contributes to reviewed, provisional or no aggregates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoringTier {
    /// Verified references and grades reviewed by a human.
    Reviewed,
    /// Scorable references with provisional grades or partial references.
    Provisional,
    /// Ambiguous or unresolved evidence.
    Excluded,
}

impl ScoringTier {
    const fn from_query(query: &Query) -> Self {
        match (query.relevance, query.references) {
            (Relevance::Reviewed, References::Verified) => Self::Reviewed,
            (
                Relevance::Reviewed | Relevance::Provisional,
                References::Verified | References::Partial,
            ) => Self::Provisional,
            _ => Self::Excluded,
        }
    }
}

/// Grade-2 results for one query, at the configured rank cutoff.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Number of grade-2 targets included in the denominator.
    pub relevant: usize,
    /// Number of distinct grade-2 targets matched.
    pub matched: usize,
    /// Fraction of the denominator matched.
    pub recall: f64,
    /// Whether any grade-2 target was matched.
    pub hit: bool,
    /// Reciprocal rank of the first matching result, or zero on a miss.
    pub reciprocal_rank: f64,
}

/// Metrics with both documented denominators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoringView {
    /// Metrics over all expected grade-2 targets, including unavailable ones.
    pub end_to_end: Metrics,
    /// Metrics over available targets only. Absent when none were available.
    pub conditional: Option<Metrics>,
}

/// Coverage evidence and scores for one query.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryScore {
    /// Reporting tier for the query.
    pub tier: ScoringTier,
    /// One assessment per judgement, preserving dataset order.
    pub assessments: Vec<Assessment>,
    /// Scores over distinct judged grade-2 pages, when the query is scorable.
    pub page: Option<ScoringView>,
    /// Scores over grade-2 judgements, omitted for page-only queries.
    pub section: Option<ScoringView>,
    /// End-to-end section nDCG with linear gains for grades 1 and 2.
    /// Omitted when section scoring does not apply.
    pub ndcg: Option<f64>,
}

/// Scores a dataset query against explicit corpus observations and ranked hits.
///
/// A missing entry in the corpus map means unknown coverage. Titles and paths
/// in hits use the dataset's canonical page names and section format. Duplicate
/// hits occupy their original ranks. Revision mismatches are recorded
/// independently of section availability. Returns `None` for an unknown ID.
#[must_use]
pub fn score_query(
    dataset: &Dataset,
    query_id: &str,
    corpus: &BTreeMap<&str, CorpusTitle<'_>>,
    hits: &[Hit<'_>],
    k: NonZeroU32,
) -> Option<QueryScore> {
    let query = dataset
        .queries()
        .iter()
        .find(|query| query.id == query_id)?;
    let assessments: Vec<_> = query
        .judgements
        .iter()
        .map(|judgement| {
            assess(
                dataset,
                judgement,
                corpus.get(judgement.page.as_str()).copied(),
            )
        })
        .collect();
    let tier = ScoringTier::from_query(query);
    let mut score = QueryScore {
        tier,
        assessments,
        page: None,
        section: None,
        ndcg: None,
    };
    if tier == ScoringTier::Excluded {
        return Some(score);
    }
    score.page = score_pages(query, &score.assessments, hits, k);
    if query
        .judgements
        .iter()
        .any(|judgement| judgement.grade == ESSENTIAL && judgement.section.is_some())
    {
        let matches = match_sections(query, &score.assessments, hits, k);
        score.section = score_sections(query, &score.assessments, &matches);
        score.ndcg = score_ndcg(query, &matches, k);
    }
    Some(score)
}

fn assess(
    dataset: &Dataset,
    judgement: &Judgement,
    observation: Option<CorpusTitle<'_>>,
) -> Assessment {
    let (availability, observed_revision) = match observation {
        Some(CorpusTitle::Present { revision, sections }) => (
            section_availability(judgement.section.as_deref(), sections),
            revision,
        ),
        Some(CorpusTitle::Missing) => (Availability::PageMissing, None),
        Some(CorpusTitle::RedirectTargetMissing(_)) => (Availability::RedirectTargetMissing, None),
        None => (Availability::Unknown, None),
    };
    let expected_revision = dataset
        .pages()
        .iter()
        .find(|page| page.title == judgement.page)
        .and_then(|page| page.oldid);
    Assessment {
        availability,
        revision: RevisionMatch::compare(expected_revision, observed_revision),
    }
}

fn section_availability(
    path: Option<&str>,
    sections: &BTreeMap<&str, SectionContent>,
) -> Availability {
    let Some(path) = path else {
        return Availability::Available;
    };
    match sections.get(path) {
        Some(SectionContent::Usable) => Availability::Available,
        Some(SectionContent::TemplateOnly) => Availability::TemplateOnly,
        None => Availability::SectionMissing,
    }
}

fn score_pages(
    query: &Query,
    assessments: &[Assessment],
    hits: &[Hit<'_>],
    k: NonZeroU32,
) -> Option<ScoringView> {
    let mut relevant = BTreeSet::new();
    let mut available = BTreeSet::new();
    for (judgement, assessment) in query.judgements.iter().zip(assessments) {
        if judgement.grade == ESSENTIAL {
            relevant.insert(judgement.page.as_str());
            if assessment.page_is_present() {
                available.insert(judgement.page.as_str());
            }
        }
    }
    let mut matched = BTreeSet::new();
    let mut first = None;
    for (hit, rank) in hits.iter().zip(1..=k.get()) {
        if available.contains(hit.page) && matched.insert(hit.page) {
            first.get_or_insert(rank);
        }
    }
    let relevant = Count::from_values(relevant.iter());
    let available = Count::from_values(available.iter());
    let matched = Count::from_values(matched.iter());
    score_view(relevant, available, matched, first)
}

// The first rank of every matched judgement, in ranking order.
fn match_sections(
    query: &Query,
    assessments: &[Assessment],
    hits: &[Hit<'_>],
    k: NonZeroU32,
) -> Vec<(usize, u32)> {
    let mut seen = BTreeSet::new();
    let mut matches = Vec::new();
    for (hit, rank) in hits.iter().zip(1..=k.get()) {
        let matching =
            query
                .judgements
                .iter()
                .zip(assessments)
                .position(|(judgement, assessment)| {
                    assessment.availability == Availability::Available
                        && hit.page == judgement.page
                        && covers(judgement.section.as_deref(), hit.section)
                });
        if let Some(index) = matching
            && seen.insert(index)
        {
            matches.push((index, rank));
        }
    }
    matches
}

fn score_sections(
    query: &Query,
    assessments: &[Assessment],
    first_ranks: &[(usize, u32)],
) -> Option<ScoringView> {
    let essential = query
        .judgements
        .iter()
        .zip(assessments)
        .filter(|(judgement, _)| judgement.grade == ESSENTIAL);
    let relevant = Count::from_values(essential.clone());
    let available = Count::from_values(
        essential.filter(|(_, assessment)| assessment.availability == Availability::Available),
    );
    let matching = first_ranks
        .iter()
        .filter(|(index, _)| query.judgements[*index].grade == ESSENTIAL);
    let matched = Count::from_values(matching.clone());
    let first = matching.map(|(_, rank)| *rank).next();
    score_view(relevant, available, matched, first)
}

fn score_ndcg(query: &Query, matches: &[(usize, u32)], k: NonZeroU32) -> Option<f64> {
    let mut ideal_grades: Vec<_> = query
        .judgements
        .iter()
        .map(|judgement| judgement.grade)
        .filter(|grade| matches!(*grade, 1 | ESSENTIAL))
        .collect();
    ideal_grades.sort_unstable_by(|a, b| b.cmp(a));
    let ideal: f64 = ideal_grades
        .into_iter()
        .zip(1..=k.get())
        .map(|(grade, rank)| discount(grade, rank))
        .sum();
    let dcg: f64 = matches
        .iter()
        .filter_map(|(index, rank)| {
            let grade = query.judgements[*index].grade;
            matches!(grade, 1 | ESSENTIAL).then(|| discount(grade, *rank))
        })
        .sum();
    (ideal > 0.0).then(|| dcg / ideal)
}

fn discount(grade: u8, rank: u32) -> f64 {
    f64::from(grade) / (f64::from(rank) + 1.0).log2()
}

#[derive(Clone, Copy)]
struct Count {
    exact: usize,
    float: f64,
}

impl Count {
    fn from_values(values: impl Iterator) -> Self {
        values.fold(
            Self {
                exact: 0,
                float: 0.0,
            },
            |count, _| Self {
                exact: count.exact + 1,
                float: count.float + 1.0,
            },
        )
    }
}

fn score_view(
    relevant: Count,
    available: Count,
    matched: Count,
    first: Option<u32>,
) -> Option<ScoringView> {
    Some(ScoringView {
        end_to_end: metrics(relevant, matched, first)?,
        conditional: metrics(available, matched, first),
    })
}

fn metrics(relevant: Count, matched: Count, first: Option<u32>) -> Option<Metrics> {
    (relevant.exact > 0).then(|| Metrics {
        relevant: relevant.exact,
        matched: matched.exact,
        recall: matched.float / relevant.float,
        hit: matched.exact > 0,
        reciprocal_rank: first.map_or(0.0, |rank| 1.0 / f64::from(rank)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(relevance: &str, references: &str, sections: bool) -> Dataset {
        let first = sections.then_some("Tools");
        let second = sections.then_some("Supplies");
        let third = sections.then_some("Layout");
        let data = json!({
            "format_version": 1,
            "dataset_id": "synthetic-workshop",
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
            "pages": [{
                "title": "Workshop",
                "requested_as": [],
                "oldid": 7,
                "last_modified": "2026-01-01",
                "revision_basis": "footer",
                "headings_complete": true,
                "observed_sections": ["Tools", "Tools > Measuring", "Supplies", "Notes"],
                "note": null
            }, {
                "title": "Storehouse",
                "requested_as": ["Supplies shed"],
                "oldid": null,
                "last_modified": null,
                "revision_basis": "not_observed",
                "headings_complete": true,
                "observed_sections": ["Layout"],
                "note": null
            }],
            "queries": [{
                "id": "q001",
                "category": "synthetic",
                "mode": "mode_independent",
                "query": "Where are the tools and supplies?",
                "intent": "Find the resources",
                "references": references,
                "relevance": relevance,
                "tags": [],
                "judgements": if sections {
                    json!([
                        {"page": "Workshop", "section": first, "grade": 2},
                        {"page": "Workshop", "section": second, "grade": 2},
                        {"page": "Storehouse", "section": third, "grade": 2},
                        {"page": "Workshop", "section": "Notes", "grade": 1}
                    ])
                } else {
                    json!([
                        {"page": "Workshop", "section": null, "grade": 2},
                        {"page": "Storehouse", "section": null, "grade": 2}
                    ])
                },
                "notes": "Synthetic independent judgements"
            }],
            "retired_query_ids": []
        });
        Dataset::parse(&data.to_string()).unwrap()
    }

    const fn hit(page: &'static str, section: Option<&'static str>) -> Hit<'static> {
        Hit { page, section }
    }

    fn assert_score(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, found {actual}"
        );
    }

    #[test]
    fn ranks_are_preserved_and_duplicate_pages_and_judgements_count_once() {
        let dataset = fixture("reviewed", "verified", true);
        let sections = BTreeMap::from([
            ("Tools", SectionContent::Usable),
            ("Supplies", SectionContent::Usable),
            ("Notes", SectionContent::Usable),
        ]);
        let corpus = BTreeMap::from([
            (
                "Workshop",
                CorpusTitle::Present {
                    revision: Some(7),
                    sections: &sections,
                },
            ),
            ("Storehouse", CorpusTitle::Missing),
        ]);
        let hits = [
            hit("Workshop", Some("Notes")),
            hit("Workshop", Some("Tools > Measuring")),
            hit("Workshop", Some("Tools > Measuring")),
            hit("Workshop", Some("Supplies")),
            hit("Storehouse", Some("Layout")),
        ];
        let scores = score_query(
            &dataset,
            "q001",
            &corpus,
            &hits,
            NonZeroU32::new(4).unwrap(),
        )
        .unwrap();
        assert_eq!(scores.tier, ScoringTier::Reviewed);
        let page = scores.page.unwrap();
        assert_score(page.end_to_end.recall, 1.0 / 2.0);
        assert_score(page.end_to_end.reciprocal_rank, 1.0);
        let section = scores.section.unwrap();
        assert_score(section.end_to_end.recall, 2.0 / 3.0);
        assert_score(section.end_to_end.reciprocal_rank, 1.0 / 2.0);
        assert!(section.end_to_end.hit);
        assert_score(section.conditional.unwrap().recall, 1.0);
        let expected_dcg = 1.0 + 2.0 / 3.0f64.log2() + 2.0 / 5.0f64.log2();
        let expected_ideal = 2.0 + 2.0 / 3.0f64.log2() + 1.0 + 1.0 / 5.0f64.log2();
        assert_score(scores.ndcg.unwrap(), expected_dcg / expected_ideal);
    }

    #[test]
    fn missing_and_template_only_sections_keep_their_pages_available() {
        let dataset = fixture("provisional", "verified", true);
        let sections = BTreeMap::from([("Tools", SectionContent::TemplateOnly)]);
        let corpus = BTreeMap::from([
            (
                "Workshop",
                CorpusTitle::Present {
                    revision: Some(8),
                    sections: &sections,
                },
            ),
            (
                "Storehouse",
                CorpusTitle::RedirectTargetMissing("Remote storehouse"),
            ),
        ]);
        let hits = [
            hit("Workshop", Some("Tools")),
            hit("Storehouse", Some("Layout")),
        ];
        let scores = score_query(&dataset, "q001", &corpus, &hits, NonZeroU32::MIN).unwrap();
        assert_eq!(scores.tier, ScoringTier::Provisional);
        assert_eq!(
            scores.assessments[0].availability,
            Availability::TemplateOnly
        );
        assert_eq!(
            scores.assessments[1].availability,
            Availability::SectionMissing
        );
        assert_eq!(
            scores.assessments[2].availability,
            Availability::RedirectTargetMissing
        );
        assert_eq!(
            scores.assessments[0].revision,
            RevisionMatch::Mismatch {
                expected: 7,
                observed: 8
            }
        );
        let page = scores.page.unwrap();
        assert_score(page.end_to_end.recall, 0.5);
        assert_score(page.conditional.unwrap().recall, 1.0);
        let section = scores.section.unwrap();
        assert_score(section.end_to_end.recall, 0.0);
        assert!(!section.end_to_end.hit);
        assert_eq!(section.conditional, None);
    }

    #[test]
    fn unavailable_hits_cannot_count_and_revision_drift_does_not_hide_usable_sections() {
        let dataset = fixture("reviewed", "verified", true);
        let sections = BTreeMap::from([("Tools", SectionContent::Usable)]);
        let corpus = BTreeMap::from([
            (
                "Workshop",
                CorpusTitle::Present {
                    revision: Some(9),
                    sections: &sections,
                },
            ),
            ("Storehouse", CorpusTitle::Missing),
        ]);
        let hits = [
            hit("Storehouse", Some("Layout")),
            hit("Workshop", Some("Tools")),
        ];
        let scores = score_query(
            &dataset,
            "q001",
            &corpus,
            &hits,
            NonZeroU32::new(2).unwrap(),
        )
        .unwrap();
        assert_eq!(scores.assessments[0].availability, Availability::Available);
        assert_eq!(
            scores.assessments[0].revision,
            RevisionMatch::Mismatch {
                expected: 7,
                observed: 9
            }
        );
        assert_eq!(
            scores.assessments[2].availability,
            Availability::PageMissing
        );
        let section = scores.section.unwrap();
        assert_eq!(section.end_to_end.matched, 1);
        assert_score(section.end_to_end.reciprocal_rank, 0.5);
        assert_eq!(section.conditional.unwrap().relevant, 1);
    }

    #[test]
    fn whole_page_hits_do_not_satisfy_section_judgements() {
        let dataset = fixture("reviewed", "verified", true);
        let sections = BTreeMap::from([("Tools", SectionContent::Usable)]);
        let corpus = BTreeMap::from([(
            "Workshop",
            CorpusTitle::Present {
                revision: Some(7),
                sections: &sections,
            },
        )]);
        let hits = [hit("Workshop", None), hit("Workshop", Some("Tools"))];
        let scores = score_query(&dataset, "q001", &corpus, &hits, NonZeroU32::MIN).unwrap();
        assert!(scores.page.unwrap().end_to_end.hit);
        assert!(!scores.section.unwrap().end_to_end.hit);
    }

    #[test]
    fn page_only_and_excluded_queries_have_no_section_scores() {
        let corpus = BTreeMap::new();
        let page_only = fixture("reviewed", "partial", false);
        let scores = score_query(&page_only, "q001", &corpus, &[], NonZeroU32::MIN).unwrap();
        assert_eq!(scores.tier, ScoringTier::Provisional);
        assert_eq!(scores.assessments[0].availability, Availability::Unknown);
        assert!(scores.page.is_some());
        assert_eq!(scores.section, None);
        assert_eq!(scores.ndcg, None);

        let ambiguous = fixture("ambiguous", "verified", true);
        let scores = score_query(&ambiguous, "q001", &corpus, &[], NonZeroU32::MIN).unwrap();
        assert_eq!(scores.tier, ScoringTier::Excluded);
        assert_eq!(scores.page, None);
        assert_eq!(scores.section, None);
    }
}
