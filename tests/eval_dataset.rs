//! Structural checks for the Wiki retrieval evaluation dataset.
//!
//! These tests read `eval/wiki-retrieval/dataset.v1.json` and enforce the format
//! documented in `eval/wiki-retrieval/README.md`. They never touch the network
//! and do not need a retrieval engine.
//!
//! Two kinds of assertion live here and are kept apart. Format checks (unique
//! ids, judged pages recorded, non-overlapping judgements) describe any dataset
//! in this format. Scope checks (query count, required categories, mode mix)
//! describe only the initial 0.1.0 benchmark and must not be carried over as
//! limits when the format types move into a library module.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

const MAX_FIELD_CHARS: usize = 500;
const REQUIRED_CATEGORIES: [&str; 7] = [
    "exact_name",
    "multi_entity",
    "game_mechanic",
    "quest",
    "skilling",
    "hardcore_safety",
    "money_making",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dataset {
    format_version: u32,
    #[serde(rename = "dataset_id")]
    id: String,
    #[serde(rename = "dataset_version")]
    version: String,
    created: String,
    source: Source,
    modes: std::collections::BTreeMap<String, String>,
    grades: std::collections::BTreeMap<String, String>,
    pages: Vec<Page>,
    queries: Vec<Query>,
    retired_query_ids: Vec<Retired>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retired {
    id: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    site: String,
    observed_on: String,
    method: String,
    licence_note: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    title: String,
    requested_as: Vec<String>,
    oldid: Option<u64>,
    last_modified: Option<String>,
    revision_basis: RevisionBasis,
    headings_complete: bool,
    observed_sections: Vec<String>,
    note: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RevisionBasis {
    Footer,
    NotObserved,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    id: String,
    category: String,
    mode: String,
    #[serde(rename = "query")]
    text: String,
    intent: String,
    references: References,
    relevance: Relevance,
    tags: Vec<String>,
    judgements: Vec<Judgement>,
    notes: Option<String>,
}

/// Whether the judged pages and sections were observed to exist.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum References {
    Verified,
    Partial,
    Unverified,
}

/// How far the relevance grades have been checked.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Relevance {
    Provisional,
    Reviewed,
    Ambiguous,
    Unresolved,
}

impl Relevance {
    fn is_scorable(self) -> bool {
        matches!(self, Self::Provisional | Self::Reviewed)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Judgement {
    page: String,
    section: Option<String>,
    grade: u8,
}

fn load() -> Dataset {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/wiki-retrieval/dataset.v1.json");
    let text = fs::read_to_string(&path).expect("dataset file should be readable");
    serde_json::from_str(&text).expect("dataset should match the v1 format")
}

fn assert_filled(name: &str, value: &str) {
    assert!(!value.trim().is_empty(), "{name} must not be blank");
}

fn page<'a>(dataset: &'a Dataset, title: &str) -> Option<&'a Page> {
    dataset.pages.iter().find(|p| p.title == title)
}

#[test]
fn header_is_complete_for_format_version_1() {
    let dataset = load();

    assert_eq!(dataset.format_version, 1);
    assert_filled("dataset_id", &dataset.id);
    assert_filled("dataset_version", &dataset.version);
    assert!(dataset.source.site.starts_with("https://"));
    assert_filled("method", &dataset.source.method);
    assert_filled("licence_note", &dataset.source.licence_note);
}

/// A date as the README documents it: `YYYY-MM-DD`. The shape is enough here;
/// whether the date exists on the calendar does not matter to the format.
fn is_dated(text: &str) -> bool {
    text.len() == 10
        && text.char_indices().all(|(index, character)| match index {
            4 | 7 => character == '-',
            _ => character.is_ascii_digit(),
        })
}

#[test]
fn dates_have_the_documented_shape() {
    let dataset = load();

    assert!(is_dated(&dataset.created), "created is not YYYY-MM-DD");
    assert!(
        is_dated(&dataset.source.observed_on),
        "observed_on is not YYYY-MM-DD"
    );
    for page in &dataset.pages {
        if let Some(modified) = &page.last_modified {
            assert!(is_dated(modified), "{} last_modified", page.title);
        }
    }
}

/// Scope check for the initial 0.1.0 benchmark only: its identity and the two
/// relevance grades it defines. Another dataset may declare other grades.
#[test]
fn initial_benchmark_declares_its_identity_and_grades() {
    let dataset = load();

    assert_eq!(dataset.id, "osrs-wiki-retrieval");
    assert_eq!(
        dataset
            .grades
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
}

/// Scope check for the initial 0.1.0 benchmark only. The 15 to 25 range is not
/// a limit of the format: later dataset versions may be larger.
#[test]
fn initial_benchmark_query_count_and_categories_cover_the_planned_scope() {
    let dataset = load();

    assert!(
        (15..=25).contains(&dataset.queries.len()),
        "expected 15 to 25 queries, found {}",
        dataset.queries.len()
    );
    for category in REQUIRED_CATEGORIES {
        assert!(
            dataset.queries.iter().any(|q| q.category == category),
            "no query in category {category}"
        );
    }
    for query in &dataset.queries {
        assert!(
            REQUIRED_CATEGORIES.contains(&query.category.as_str()),
            "{} has unknown category {}",
            query.id,
            query.category
        );
    }
}

#[test]
fn query_ids_and_texts_are_unique_and_well_formed() {
    let dataset = load();
    let mut ids = HashSet::new();
    let mut texts = HashSet::new();

    for query in &dataset.queries {
        let digits = query.id.strip_prefix('q').unwrap_or("");
        assert!(
            digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit()),
            "bad id {}",
            query.id
        );
        assert!(ids.insert(query.id.as_str()), "duplicate id {}", query.id);
        assert!(
            texts.insert(query.text.to_lowercase()),
            "duplicate query text for {}",
            query.id
        );
        assert!(
            !query.text.trim().is_empty(),
            "{} has an empty query",
            query.id
        );
        assert!(
            !query.intent.trim().is_empty(),
            "{} has no intent",
            query.id
        );
    }
}

#[test]
fn judgements_and_recorded_pages_reference_each_other() {
    let dataset = load();

    for query in &dataset.queries {
        for judgement in &query.judgements {
            let recorded = page(&dataset, &judgement.page).unwrap_or_else(|| {
                panic!("{} judges unrecorded page {}", query.id, judgement.page)
            });
            if let Some(section) = &judgement.section {
                assert!(
                    recorded.observed_sections.contains(section),
                    "{} judges section {section:?} not observed on {}",
                    query.id,
                    recorded.title
                );
            }
        }
    }
    for page in &dataset.pages {
        assert!(
            dataset
                .queries
                .iter()
                .flat_map(|q| &q.judgements)
                .any(|j| j.page == page.title),
            "page {} is recorded but never judged",
            page.title
        );
    }
}

#[test]
fn judgement_grades_are_declared_in_the_dataset() {
    let dataset = load();

    for query in &dataset.queries {
        for judgement in &query.judgements {
            assert!(
                dataset.grades.contains_key(&judgement.grade.to_string()),
                "{} has undeclared grade {}",
                query.id,
                judgement.grade
            );
        }
    }
}

/// True if `outer` names the same section as `inner` or one of its ancestors.
fn covers(outer: Option<&str>, inner: Option<&str>) -> bool {
    match (outer, inner) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(outer), Some(inner)) => {
            inner == outer
                || inner
                    .strip_prefix(outer)
                    .is_some_and(|rest| rest.starts_with(" > "))
        }
    }
}

#[test]
fn judgements_never_overlap_so_one_result_cannot_count_twice() {
    let dataset = load();

    for query in &dataset.queries {
        for (i, a) in query.judgements.iter().enumerate() {
            for b in &query.judgements[i + 1..] {
                let overlap = a.page == b.page
                    && (covers(a.section.as_deref(), b.section.as_deref())
                        || covers(b.section.as_deref(), a.section.as_deref()));
                assert!(
                    !overlap,
                    "{} has overlapping judgements on {} ({:?} and {:?})",
                    query.id, a.page, a.section, b.section
                );
            }
        }
    }
}

#[test]
fn scorable_queries_have_an_essential_judgement_and_checked_references() {
    let dataset = load();

    for query in &dataset.queries {
        if query.relevance.is_scorable() {
            assert!(
                query.judgements.iter().any(|j| j.grade == 2),
                "{} is scorable but has no essential judgement",
                query.id
            );
            assert_ne!(
                query.references,
                References::Unverified,
                "{} is scorable but its references are unverified",
                query.id
            );
        }
    }
}

#[test]
fn queries_needing_explanation_have_notes() {
    let dataset = load();

    for query in &dataset.queries {
        let plain = query.references == References::Verified && query.relevance.is_scorable();
        let has_notes = query.notes.as_deref().is_some_and(|n| !n.trim().is_empty());
        assert!(
            plain || has_notes,
            "{} needs notes ({:?}, {:?})",
            query.id,
            query.references,
            query.relevance
        );
    }
}

/// Scope check for the initial 0.1.0 benchmark only: it keeps one unresolved
/// example (the Hardcore Worlds economy query) visible. Not a format rule.
#[test]
fn initial_benchmark_records_an_unresolved_example() {
    let dataset = load();

    assert!(
        dataset
            .queries
            .iter()
            .any(|q| q.relevance == Relevance::Unresolved),
        "dataset should record at least one unresolved example"
    );
}

#[test]
fn query_modes_are_declared_and_contrast_queries_are_tagged() {
    let dataset = load();

    for query in &dataset.queries {
        assert!(
            dataset.modes.contains_key(&query.mode),
            "{} has undeclared mode {}",
            query.id,
            query.mode
        );
        if query.mode == "hardcore_ironman" {
            assert!(
                query.tags.iter().any(|t| t == "disambiguation"),
                "{} concerns Hardcore Ironman and must be tagged as disambiguation",
                query.id
            );
        }
    }
}

/// Scope check for the initial 0.1.0 benchmark only: Hardcore Worlds is the
/// project's target mode, so it must outnumber the Hardcore Ironman contrast
/// queries. Later datasets may be composed differently.
#[test]
fn initial_benchmark_focuses_on_hardcore_worlds() {
    let dataset = load();
    let worlds = dataset
        .queries
        .iter()
        .filter(|q| q.mode == "hardcore_worlds")
        .count();
    let ironman = dataset
        .queries
        .iter()
        .filter(|q| q.mode == "hardcore_ironman")
        .count();
    assert!(
        worlds >= 4,
        "expected several Hardcore Worlds queries, found {worlds}"
    );
    assert!(
        worlds > ironman,
        "Hardcore Ironman must stay a minority of mode queries"
    );
}

#[test]
fn retired_ids_are_explained_and_never_reused() {
    let dataset = load();

    for retired in &dataset.retired_query_ids {
        assert_filled(&format!("{} reason", retired.id), &retired.reason);
        assert!(
            dataset.queries.iter().all(|q| q.id != retired.id),
            "{} is retired but still present",
            retired.id
        );
    }
}

#[test]
fn recorded_pages_have_unique_titles_and_unique_sections() {
    let dataset = load();
    let mut titles = HashSet::new();

    for page in &dataset.pages {
        assert!(
            titles.insert(page.title.as_str()),
            "duplicate page {}",
            page.title
        );
        assert!(
            !page.observed_sections.is_empty(),
            "{} has no sections",
            page.title
        );
        let unique: HashSet<_> = page.observed_sections.iter().collect();
        assert_eq!(unique.len(), page.observed_sections.len(), "{}", page.title);
    }
}

#[test]
fn page_revision_fields_agree_with_each_other() {
    let dataset = load();

    for page in &dataset.pages {
        assert_eq!(
            page.oldid.is_some(),
            page.last_modified.is_some(),
            "{} must record oldid and date together",
            page.title
        );
        assert_eq!(
            page.oldid.is_none(),
            page.revision_basis == RevisionBasis::NotObserved,
            "{} revision basis does not match its revision data",
            page.title
        );
    }
}

#[test]
fn incomplete_heading_lists_are_explained() {
    let dataset = load();

    for page in &dataset.pages {
        assert!(
            page.headings_complete || page.note.as_deref().is_some_and(|n| !n.trim().is_empty()),
            "{} has an incomplete heading list and needs a note",
            page.title
        );
    }
}

/// Every free-text string in the dataset, one schema level at a time. Kept
/// explicit so that a new field has to be added here deliberately.
fn free_text_fields(dataset: &Dataset) -> Vec<&str> {
    let mut fields: Vec<&str> = vec![
        &dataset.id,
        &dataset.version,
        &dataset.created,
        &dataset.source.site,
        &dataset.source.observed_on,
        &dataset.source.method,
        &dataset.source.licence_note,
    ];
    fields.extend(dataset.grades.values().map(String::as_str));
    fields.extend(dataset.modes.values().map(String::as_str));
    for retired in &dataset.retired_query_ids {
        fields.push(&retired.id);
        fields.push(&retired.reason);
    }
    for page in &dataset.pages {
        fields.push(&page.title);
        fields.extend(page.requested_as.iter().map(String::as_str));
        fields.extend(page.last_modified.as_deref());
        fields.extend(page.observed_sections.iter().map(String::as_str));
        fields.extend(page.note.as_deref());
    }
    for query in &dataset.queries {
        fields.push(&query.id);
        fields.push(&query.category);
        fields.push(&query.text);
        fields.push(&query.intent);
        fields.push(&query.mode);
        fields.extend(query.tags.iter().map(String::as_str));
        fields.extend(query.notes.as_deref());
        for judgement in &query.judgements {
            fields.push(&judgement.page);
            fields.extend(judgement.section.as_deref());
        }
    }
    fields
}

/// Bounds the length of every free-text string in the dataset. This is a
/// guard against pasted passages, not proof of provenance: a short string can
/// still be copied text, so the dataset's originality is confirmed by manual
/// review (see "Extending the dataset" in the README).
#[test]
fn every_free_text_string_is_length_bounded() {
    let dataset = load();

    for field in free_text_fields(&dataset) {
        assert!(
            field.chars().count() <= MAX_FIELD_CHARS,
            "field is too long: {field:.60}"
        );
    }
}
