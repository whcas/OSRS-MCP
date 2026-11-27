//! Repository checks for the committed Wiki retrieval benchmark.
use std::fs;
use std::path::PathBuf;

use osrs_mcp::eval::{Dataset, Relevance};

const REQUIRED_CATEGORIES: [&str; 7] = [
    "exact_name",
    "multi_entity",
    "game_mechanic",
    "quest",
    "skilling",
    "hardcore_safety",
    "money_making",
];

fn load() -> Dataset {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/wiki-retrieval/dataset.v1.json");
    let text = fs::read_to_string(&path).expect("dataset file should be readable");
    Dataset::parse(&text).expect("dataset should match the v1 format")
}

#[test]
fn committed_benchmark_matches_the_dataset_format() {
    let _dataset = load();
}

/// Scope check for the initial 0.1.0 benchmark only: its identity and the two
/// relevance grades it defines. Another dataset may declare other grades.
#[test]
fn initial_benchmark_declares_its_identity_and_grades() {
    let dataset = load();

    assert_eq!(dataset.id(), "osrs-wiki-retrieval");
    assert_eq!(
        dataset
            .grades()
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
        (15..=25).contains(&dataset.queries().len()),
        "expected 15 to 25 queries, found {}",
        dataset.queries().len()
    );
    for category in REQUIRED_CATEGORIES {
        assert!(
            dataset.queries().iter().any(|q| q.category == category),
            "no query in category {category}"
        );
    }
    for query in dataset.queries() {
        assert!(
            REQUIRED_CATEGORIES.contains(&query.category.as_str()),
            "{} has unknown category {}",
            query.id,
            query.category
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
            .queries()
            .iter()
            .any(|q| q.relevance == Relevance::Unresolved),
        "dataset should record at least one unresolved example"
    );
}

/// Scope check for the initial 0.1.0 benchmark only: Hardcore Worlds is the
/// project's target mode, so it must outnumber the Hardcore Ironman contrast
/// queries. Later datasets may be composed differently.
#[test]
fn initial_benchmark_focuses_on_hardcore_worlds() {
    let dataset = load();
    let worlds = dataset
        .queries()
        .iter()
        .filter(|q| q.mode == "hardcore_worlds")
        .count();
    let ironman = dataset
        .queries()
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
