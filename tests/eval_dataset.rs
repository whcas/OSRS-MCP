//! Structural checks for the Wiki retrieval evaluation dataset.
//!
//! These tests read `eval/wiki-retrieval/dataset.v1.json` and enforce the format
//! documented in `eval/wiki-retrieval/README.md`. They never touch the network
//! and do not need a retrieval engine.

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
fn header_identifies_format_and_dataset() {
    let dataset = load();

    assert_eq!(dataset.format_version, 1);
    assert_eq!(dataset.id, "osrs-wiki-retrieval");
    assert_filled("dataset_version", &dataset.version);
    assert_filled("created", &dataset.created);
    assert!(dataset.source.site.starts_with("https://"));
    assert_filled("observed_on", &dataset.source.observed_on);
    assert_filled("method", &dataset.source.method);
    assert_filled("licence_note", &dataset.source.licence_note);
    assert_eq!(
        dataset
            .grades
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
}

#[test]
fn query_count_and_categories_cover_the_planned_scope() {
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
fn judgements_reference_recorded_pages_and_observed_sections() {
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
            assert!(
                matches!(judgement.grade, 1 | 2),
                "{} has grade {}",
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
        (Some(outer), Some(inner)) => inner == outer || inner.starts_with(&format!("{outer} > ")),
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
    assert!(
        dataset
            .queries
            .iter()
            .any(|q| q.relevance == Relevance::Unresolved),
        "dataset should record at least one unresolved example"
    );
}

#[test]
fn modes_are_declared_and_hardcore_worlds_is_the_focus() {
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
fn pages_are_unique_and_revisions_are_consistent() {
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
        assert!(
            page.headings_complete || page.note.as_deref().is_some_and(|n| !n.trim().is_empty()),
            "{} has an incomplete heading list and needs a note",
            page.title
        );
        let unique: HashSet<_> = page.observed_sections.iter().collect();
        assert_eq!(unique.len(), page.observed_sections.len(), "{}", page.title);
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
fn every_text_field_is_short_enough_to_exclude_wiki_excerpts() {
    let dataset = load();
    let mut fields: Vec<&str> = vec![&dataset.source.method, &dataset.source.licence_note];
    fields.extend(dataset.grades.values().map(String::as_str));
    fields.extend(dataset.modes.values().map(String::as_str));
    for page in &dataset.pages {
        fields.push(&page.title);
        fields.extend(page.requested_as.iter().map(String::as_str));
        fields.extend(page.observed_sections.iter().map(String::as_str));
        fields.extend(page.note.as_deref());
    }
    for query in &dataset.queries {
        fields.push(&query.text);
        fields.push(&query.intent);
        fields.push(&query.mode);
        fields.extend(query.tags.iter().map(String::as_str));
        fields.extend(query.notes.as_deref());
    }

    for field in fields {
        assert!(
            field.chars().count() <= MAX_FIELD_CHARS,
            "field is too long: {field:.60}"
        );
    }
}
