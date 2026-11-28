//! Aggregates a run's per-query scores into the figures a report shows.
//!
//! Every figure is a mean over queries (a macro average), grouped by scoring
//! tier so provisional and reviewed results are never mixed, and then overall,
//! per category and per mode. Coverage is counted over essential judgements,
//! never dropped: an unavailable target stays in every end-to-end figure and
//! is reported as a coverage gap, not a ranking failure.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::time::Duration;

use super::run::{EvaluationRun, QueryRun, SearchOutcome, TitleOutcome};
use super::scorer::{Availability, Metrics, QueryScore, RevisionMatch, ScoringTier, ScoringView};
use super::{Dataset, Query};

const ESSENTIAL: u8 = 2;

/// What a group of queries has in common.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Every query of the tier.
    Overall,
    /// Queries of one dataset category.
    Category(String),
    /// Queries about one game mode.
    Mode(String),
}

/// Means over the queries that have the figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Means {
    /// How many queries the means cover.
    pub queries: usize,
    /// Mean recall.
    pub recall: f64,
    /// Fraction of queries with at least one essential target in the top k.
    pub hit_rate: f64,
    /// Mean reciprocal rank.
    pub mrr: f64,
}

/// Means under both denominators of the coverage-aware evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewMeans {
    /// Over all essential targets, unavailable ones counted as not found.
    pub end_to_end: Means,
    /// Over available essential targets only. Absent when no query had any.
    pub conditional: Option<Means>,
}

/// Figures at one rank cutoff.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutoffSummary {
    /// The cutoff.
    pub k: NonZeroU32,
    /// Page-level means.
    pub page: Option<ViewMeans>,
    /// Section-level means over queries that judge sections.
    pub section: Option<ViewMeans>,
    /// Mean end-to-end section nDCG and the number of queries it covers.
    pub ndcg: Option<(usize, f64)>,
}

/// How many essential judgements the corpus can and cannot support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Coverage {
    /// Essential judgements counted.
    pub judgements: usize,
    /// Everything needed for the judgement is in the corpus.
    pub available: usize,
    /// The judged page is not in the corpus.
    pub page_missing: usize,
    /// The page is a redirect whose target is not in the corpus.
    pub redirect_target_missing: usize,
    /// The page is in the corpus without the judged section.
    pub section_missing: usize,
    /// The section exists, but only as unexpanded template calls.
    pub template_only: usize,
    /// The corpus could not say.
    pub unknown: usize,
    /// Judgements on a page whose imported revision differs from the dataset's.
    pub revision_mismatches: usize,
}

/// What searching cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Costs {
    /// Queries that were searched.
    pub searched: usize,
    /// Mean characters of text returned per query at `k`.
    pub mean_response_chars: f64,
    /// The most characters returned for one query.
    pub max_response_chars: usize,
    /// Median wall-clock time of one search.
    pub median_latency: Duration,
    /// 95th percentile (nearest rank) wall-clock time of one search.
    pub p95_latency: Duration,
}

/// The figures for one group of one tier.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupSummary {
    /// The tier the queries are in.
    pub tier: ScoringTier,
    /// What the queries have in common.
    pub group: Group,
    /// Queries in the group.
    pub queries: usize,
    /// Coverage over their essential judgements.
    pub coverage: Coverage,
    /// Figures per rank cutoff, smallest first.
    pub cutoffs: Vec<CutoffSummary>,
    /// Costs; absent when no query in the group was searched.
    pub costs: Option<Costs>,
}

/// How the corpus resolves the titles that exact-name and redirect queries use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TitleResolution {
    /// Resolves to the canonical page.
    pub canonical: usize,
    /// Resolves to a different page, or the canonical page is absent.
    pub elsewhere: usize,
    /// A redirect whose target was not imported.
    pub target_missing: usize,
    /// Not imported.
    pub absent: usize,
    /// Redirects that could not be followed to one page.
    pub unknown: usize,
}

/// The aggregated results of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// One entry per tier and group that has queries, tiers first.
    pub groups: Vec<GroupSummary>,
    /// Ids of queries left out of scores because their evidence is ambiguous
    /// or unresolved.
    pub excluded: Vec<String>,
    /// Ids of scorable queries whose text cannot be searched.
    pub rejected: Vec<String>,
    /// Title resolution for the exact-name and redirect queries.
    pub titles: TitleResolution,
}

/// Aggregates `run` over `dataset`, which must be the dataset it ran.
#[must_use]
pub fn summarise(dataset: &Dataset, run: &EvaluationRun) -> Summary {
    let pairs: Vec<(&Query, &QueryRun)> = dataset.queries().iter().zip(run.queries()).collect();
    let mut buckets: BTreeMap<(ScoringTier, Group), Vec<(&Query, &QueryRun)>> = BTreeMap::new();
    for (query, query_run) in &pairs {
        let Some(tier) = tier_of(query_run) else {
            continue;
        };
        if tier == ScoringTier::Excluded {
            continue;
        }
        for group in [
            Group::Overall,
            Group::Category(query.category.clone()),
            Group::Mode(query.mode.clone()),
        ] {
            buckets
                .entry((tier, group))
                .or_default()
                .push((query, query_run));
        }
    }
    Summary {
        groups: buckets
            .into_iter()
            .map(|((tier, group), members)| summarise_group(tier, group, &members))
            .collect(),
        excluded: ids(&pairs, |_, query_run| {
            tier_of(query_run) == Some(ScoringTier::Excluded)
        }),
        rejected: ids(&pairs, |_, query_run| {
            matches!(query_run.search, SearchOutcome::Rejected(_))
        }),
        titles: title_resolution(dataset, run),
    }
}

fn ids(pairs: &[(&Query, &QueryRun)], include: impl Fn(&Query, &QueryRun) -> bool) -> Vec<String> {
    pairs
        .iter()
        .filter(|(query, query_run)| include(query, query_run))
        .map(|(query, _)| query.id.clone())
        .collect()
}

fn tier_of(query_run: &QueryRun) -> Option<ScoringTier> {
    query_run.scores.first().map(|entry| entry.score.tier)
}

fn summarise_group(
    tier: ScoringTier,
    group: Group,
    members: &[(&Query, &QueryRun)],
) -> GroupSummary {
    let cutoffs: Vec<NonZeroU32> = members.first().map_or_else(Vec::new, |(_, query_run)| {
        query_run.scores.iter().map(|entry| entry.k).collect()
    });
    GroupSummary {
        tier,
        group,
        queries: members.len(),
        coverage: coverage(members),
        cutoffs: cutoffs
            .into_iter()
            .map(|k| summarise_cutoff(k, members))
            .collect(),
        costs: costs(members),
    }
}

fn scores_at<'a>(
    k: NonZeroU32,
    members: &'a [(&Query, &QueryRun)],
) -> impl Iterator<Item = &'a QueryScore> {
    members.iter().filter_map(move |(_, query_run)| {
        query_run
            .scores
            .iter()
            .find(|entry| entry.k == k)
            .map(|entry| &entry.score)
    })
}

fn summarise_cutoff(k: NonZeroU32, members: &[(&Query, &QueryRun)]) -> CutoffSummary {
    let ndcg: Vec<f64> = scores_at(k, members)
        .filter_map(|score| score.ndcg)
        .collect();
    CutoffSummary {
        k,
        page: view_means(scores_at(k, members).filter_map(|score| score.page.as_ref())),
        section: view_means(scores_at(k, members).filter_map(|score| score.section.as_ref())),
        ndcg: (!ndcg.is_empty()).then(|| (ndcg.len(), mean(&ndcg))),
    }
}

fn view_means<'a>(views: impl Iterator<Item = &'a ScoringView>) -> Option<ViewMeans> {
    let views: Vec<_> = views.collect();
    let end_to_end = means(views.iter().map(|view| &view.end_to_end))?;
    Some(ViewMeans {
        end_to_end,
        conditional: means(views.iter().filter_map(|view| view.conditional.as_ref())),
    })
}

fn means<'a>(metrics: impl Iterator<Item = &'a Metrics>) -> Option<Means> {
    let metrics: Vec<_> = metrics.collect();
    if metrics.is_empty() {
        return None;
    }
    let recall: Vec<f64> = metrics.iter().map(|entry| entry.recall).collect();
    let rank: Vec<f64> = metrics.iter().map(|entry| entry.reciprocal_rank).collect();
    let hits: Vec<f64> = metrics
        .iter()
        .map(|entry| f64::from(u8::from(entry.hit)))
        .collect();
    Some(Means {
        queries: metrics.len(),
        recall: mean(&recall),
        hit_rate: mean(&hits),
        mrr: mean(&rank),
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts are far below the 2^52 where f64 stops being exact"
)]
fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn coverage(members: &[(&Query, &QueryRun)]) -> Coverage {
    let mut coverage = Coverage::default();
    for (query, query_run) in members {
        let Some(entry) = query_run.scores.first() else {
            continue;
        };
        for (judgement, assessment) in query.judgements.iter().zip(&entry.score.assessments) {
            if judgement.grade != ESSENTIAL {
                continue;
            }
            coverage.judgements += 1;
            let state = match assessment.availability {
                Availability::Available => &mut coverage.available,
                Availability::PageMissing => &mut coverage.page_missing,
                Availability::RedirectTargetMissing => &mut coverage.redirect_target_missing,
                Availability::SectionMissing => &mut coverage.section_missing,
                Availability::TemplateOnly => &mut coverage.template_only,
                Availability::Unknown => &mut coverage.unknown,
            };
            *state += 1;
            if matches!(assessment.revision, RevisionMatch::Mismatch { .. }) {
                coverage.revision_mismatches += 1;
            }
        }
    }
    coverage
}

fn costs(members: &[(&Query, &QueryRun)]) -> Option<Costs> {
    let searched: Vec<&QueryRun> = members
        .iter()
        .map(|(_, query_run)| *query_run)
        .filter(|query_run| query_run.search == SearchOutcome::Searched)
        .collect();
    if searched.is_empty() {
        return None;
    }
    let sizes: Vec<usize> = searched.iter().map(|run| run.response_chars()).collect();
    let mut latencies: Vec<Duration> = searched.iter().map(|run| run.latency).collect();
    latencies.sort_unstable();
    #[expect(
        clippy::cast_precision_loss,
        reason = "character counts are far below the 2^52 where f64 stops being exact"
    )]
    let mean_response_chars = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;
    Some(Costs {
        searched: searched.len(),
        mean_response_chars,
        max_response_chars: sizes.iter().copied().max().unwrap_or(0),
        median_latency: percentile(&latencies, 0.5),
        p95_latency: percentile(&latencies, 0.95),
    })
}

/// The nearest-rank percentile of sorted, nonempty values.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the rank is a small positive count rounded up from a fraction of it"
    )]
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Counts how the titles behind exact-name and redirect queries resolve: the
/// judged pages' own titles and the alternative titles recorded for them.
fn title_resolution(dataset: &Dataset, run: &EvaluationRun) -> TitleResolution {
    let mut checked = BTreeSet::new();
    for (query, query_run) in dataset.queries().iter().zip(run.queries()) {
        let scorable = tier_of(query_run).is_some_and(|tier| tier != ScoringTier::Excluded);
        let tests_titles =
            query.category == "exact_name" || query.tags.iter().any(|tag| tag == "redirect");
        if scorable && tests_titles {
            for judgement in &query.judgements {
                checked.insert(judgement.page.as_str());
            }
        }
    }
    let mut resolution = TitleResolution::default();
    for page in dataset
        .pages()
        .iter()
        .filter(|page| checked.contains(page.title.as_str()))
    {
        let outcomes = std::iter::once(run.observations().title_outcome(&page.title)).chain(
            page.requested_as.iter().map(|title| {
                run.observations()
                    .redirect_outcome(title)
                    .unwrap_or(TitleOutcome::Unknown)
            }),
        );
        for outcome in outcomes {
            let count = match outcome {
                TitleOutcome::Canonical => &mut resolution.canonical,
                TitleOutcome::Elsewhere => &mut resolution.elsewhere,
                TitleOutcome::TargetMissing => &mut resolution.target_missing,
                TitleOutcome::Absent => &mut resolution.absent,
                TitleOutcome::Unknown => &mut resolution.unknown,
            };
            *count += 1;
        }
    }
    resolution
}

/// Essential sections whose page is imported but whose text is only template
/// calls, as `(page, section)` pairs. They are limits of the imported article,
/// not titles to request.
#[must_use]
pub fn template_only_sections(dataset: &Dataset, run: &EvaluationRun) -> Vec<(String, String)> {
    let mut sections = BTreeSet::new();
    for (query, query_run) in dataset.queries().iter().zip(run.queries()) {
        let Some(entry) = query_run.scores.first() else {
            continue;
        };
        if entry.score.tier == ScoringTier::Excluded {
            continue;
        }
        for (judgement, assessment) in query.judgements.iter().zip(&entry.score.assessments) {
            if judgement.grade == ESSENTIAL
                && assessment.availability == Availability::TemplateOnly
                && let Some(section) = &judgement.section
            {
                sections.insert((judgement.page.clone(), section.clone()));
            }
        }
    }
    sections.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::super::run::run;
    use super::super::run::synthetic::{config, corpus, dataset, pages};
    use super::*;

    fn summarised(
        workshop_revision: u64,
        workshop_oldid: u64,
    ) -> (Dataset, EvaluationRun, Summary) {
        let corpus = corpus(&pages(workshop_revision));
        let dataset = dataset(workshop_oldid);
        let evaluation = run(&corpus.store, &corpus.source, &dataset, config(10)).unwrap();
        let summary = summarise(&dataset, &evaluation);
        (dataset, evaluation, summary)
    }

    fn group<'a>(summary: &'a Summary, wanted: &Group) -> &'a GroupSummary {
        summary
            .groups
            .iter()
            .find(|entry| entry.tier == ScoringTier::Provisional && &entry.group == wanted)
            .unwrap()
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, found {actual}"
        );
    }

    fn cutoff(group: &GroupSummary, k: u32) -> &CutoffSummary {
        group
            .cutoffs
            .iter()
            .find(|entry| entry.k.get() == k)
            .unwrap()
    }

    #[test]
    fn page_means_show_end_to_end_and_conditional_figures_side_by_side() {
        let (_, _, summary) = summarised(7, 7);
        let overall = group(&summary, &Group::Overall);
        assert_eq!(overall.queries, 6);
        let page = cutoff(overall, 10).page.unwrap();
        // Recall per query: 1, 1/2, 1, 0 (redirect target missing), 1, 0.
        assert_eq!(page.end_to_end.queries, 6);
        assert_close(page.end_to_end.recall, 3.5 / 6.0);
        assert_close(page.end_to_end.hit_rate, 4.0 / 6.0);
        assert_close(page.end_to_end.mrr, 4.0 / 6.0);
        // The query whose only target is unavailable has no conditional figure.
        let conditional = page.conditional.unwrap();
        assert_eq!(conditional.queries, 5);
        assert_close(conditional.recall, 4.0 / 5.0);
        assert_close(conditional.hit_rate, 4.0 / 5.0);
    }

    #[test]
    fn section_means_cover_only_queries_that_judge_sections() {
        let (_, _, summary) = summarised(7, 7);
        let section = cutoff(group(&summary, &Group::Overall), 10)
            .section
            .unwrap();
        // q001 finds its section, q002 one of two, q003 sits behind a template.
        assert_eq!(section.end_to_end.queries, 3);
        assert_close(section.end_to_end.recall, 0.5);
        assert_close(section.end_to_end.hit_rate, 2.0 / 3.0);
        let conditional = section.conditional.unwrap();
        assert_eq!(conditional.queries, 2);
        assert_close(conditional.recall, 1.0);
    }

    #[test]
    fn coverage_counts_every_essential_judgement_by_state() {
        let (_, _, summary) = summarised(7, 7);
        assert_eq!(
            group(&summary, &Group::Overall).coverage,
            Coverage {
                judgements: 7,
                available: 4,
                page_missing: 1,
                redirect_target_missing: 1,
                section_missing: 0,
                template_only: 1,
                unknown: 0,
                revision_mismatches: 0,
            }
        );
    }

    #[test]
    fn revision_mismatches_are_counted_in_coverage() {
        let (_, _, summary) = summarised(8, 7);
        assert_eq!(
            group(&summary, &Group::Overall)
                .coverage
                .revision_mismatches,
            2
        );
    }

    #[test]
    fn groups_split_by_category_and_mode() {
        let (_, _, summary) = summarised(7, 7);
        assert_eq!(
            group(&summary, &Group::Category("lookup".into())).queries,
            4
        );
        assert_eq!(
            group(&summary, &Group::Category("exact_name".into())).queries,
            2
        );
        assert_eq!(
            group(&summary, &Group::Mode("mode_independent".into())).queries,
            6
        );
        assert!(
            summary
                .groups
                .iter()
                .all(|entry| entry.tier == ScoringTier::Provisional),
            "no query is reviewed, so there is no reviewed tier"
        );
    }

    #[test]
    fn excluded_and_unsearchable_queries_are_listed_apart() {
        let (_, _, summary) = summarised(7, 7);
        assert_eq!(summary.excluded, ["q006"]);
        assert_eq!(summary.rejected, ["q007"]);
    }

    #[test]
    fn costs_cover_the_searched_queries_only() {
        let (_, evaluation, summary) = summarised(7, 7);
        let costs = group(&summary, &Group::Overall).costs.unwrap();
        assert_eq!(costs.searched, 5);
        let sizes: Vec<usize> = evaluation
            .queries()
            .iter()
            .filter(|query| query.search == SearchOutcome::Searched)
            .map(QueryRun::response_chars)
            .collect();
        assert_eq!(
            costs.max_response_chars,
            sizes.iter().copied().max().unwrap()
        );
        assert!(costs.max_response_chars > 0);
        assert!(costs.median_latency <= costs.p95_latency);
    }

    #[test]
    fn title_resolution_counts_the_titles_exact_name_and_redirect_queries_use() {
        let (_, _, summary) = summarised(7, 7);
        assert_eq!(
            summary.titles,
            TitleResolution {
                canonical: 1,
                target_missing: 1,
                ..TitleResolution::default()
            }
        );
    }

    #[test]
    fn template_only_sections_are_listed_apart_from_missing_pages() {
        let (dataset, evaluation, _) = summarised(7, 7);
        assert_eq!(
            template_only_sections(&dataset, &evaluation),
            [("Workshop".to_owned(), "Notes".to_owned())]
        );
        assert_eq!(
            evaluation.missing_pages(&dataset),
            ["Orchard", "Storehouse"]
        );
    }

    #[test]
    fn nearest_rank_percentiles() {
        let values: Vec<Duration> = (1..=20).map(Duration::from_millis).collect();
        assert_eq!(percentile(&values, 0.5), Duration::from_millis(10));
        assert_eq!(percentile(&values, 0.95), Duration::from_millis(19));
        assert_eq!(percentile(&values[..1], 0.95), Duration::from_millis(1));
    }
}
