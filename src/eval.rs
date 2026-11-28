//! Dataset format for offline Wiki retrieval evaluation.

/// Coverage-aware scoring of ranked Wiki retrieval results.
pub mod scorer;

/// Runs dataset queries against an imported corpus and scores them.
pub mod run;

/// Aggregates a run's scores by tier, category and mode.
pub mod summary;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use serde::Deserialize;
use thiserror::Error;

const MAX_FIELD_CHARS: usize = 500;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// The versioned input dataset for a Wiki retrieval evaluation.
pub struct Dataset {
    format_version: u32,
    #[serde(rename = "dataset_id")]
    id: String,
    #[serde(rename = "dataset_version")]
    version: String,
    created: String,
    source: Source,
    modes: BTreeMap<String, String>,
    grades: BTreeMap<String, String>,
    pages: Vec<Page>,
    queries: Vec<Query>,
    retired_query_ids: Vec<Retired>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retired {
    id: String,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    site: String,
    observed_on: String,
    method: String,
    licence_note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// A canonical page recorded in the benchmark.
pub struct Page {
    /// Canonical page title.
    pub title: String,
    /// Titles that resolved to this page.
    pub requested_as: Vec<String>,
    /// Permanent revision identifier, when observed.
    pub oldid: Option<u64>,
    /// Date of the observed revision.
    pub last_modified: Option<String>,
    /// How the revision was observed.
    pub revision_basis: RevisionBasis,
    /// Whether observation covered all headings.
    pub headings_complete: bool,
    /// Recorded heading paths.
    pub observed_sections: Vec<String>,
    /// Explanation of incomplete observation.
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Where a recorded revision identifier came from.
pub enum RevisionBasis {
    /// Read from the page footer.
    Footer,
    /// The reference has no observed revision.
    NotObserved,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// One query and its expected relevant results.
pub struct Query {
    /// Stable query identifier.
    pub id: String,
    /// Benchmark category.
    pub category: String,
    /// Game mode declared by the dataset.
    pub mode: String,
    #[serde(rename = "query")]
    /// Query text passed to retrieval.
    pub text: String,
    /// Reason for including this query.
    pub intent: String,
    /// Whether the judged references were observed.
    pub references: References,
    /// Review status of the relevance assessment.
    pub relevance: Relevance,
    /// Query labels.
    pub tags: Vec<String>,
    /// Expected pages and sections.
    pub judgements: Vec<Judgement>,
    /// Explanation of incomplete or uncertain assessment.
    pub notes: Option<String>,
}

/// Whether the judged pages and sections were observed to exist.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum References {
    /// All judged references were observed.
    Verified,
    /// Only some judged references were observed.
    Partial,
    /// The judged references were not observed.
    Unverified,
}

/// How far the relevance grades have been checked.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Relevance {
    /// Tentative grades from the dataset author.
    Provisional,
    /// Grades checked by a human against source text.
    Reviewed,
    /// The relevance assessment is ambiguous.
    Ambiguous,
    /// No reference could be established.
    Unresolved,
}

impl Relevance {
    const fn is_scorable(self) -> bool {
        matches!(self, Self::Provisional | Self::Reviewed)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// Relevance assigned to a page or one of its sections.
pub struct Judgement {
    /// Canonical page title.
    pub page: String,
    /// Section path, or absent for a whole-page judgement.
    pub section: Option<String>,
    /// Relevance grade declared in the dataset.
    pub grade: u8,
}

/// An invalid retrieval evaluation dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DatasetError {
    /// JSON syntax or field types do not match the dataset schema.
    #[error("invalid dataset JSON at line {line}, column {column}")]
    Json {
        /// Line reported by the parser.
        line: usize,
        /// Column reported by the parser.
        column: usize,
    },
    /// The dataset metadata fails a format rule.
    #[error("invalid dataset metadata: {0}")]
    Metadata(&'static str),
    /// A page record fails a format rule.
    #[error("invalid page record {index}: {rule}")]
    Page {
        /// One-based position in the array.
        index: usize,
        /// Which invariant failed.
        rule: &'static str,
    },
    /// A query record fails a format rule.
    #[error("invalid query record {index}: {rule}")]
    Query {
        /// One-based position in the array.
        index: usize,
        /// Which invariant failed.
        rule: &'static str,
    },
    /// A field exceeds the documented maximum size.
    #[error("dataset text field exceeds 500 characters")]
    TextTooLong,
}

impl Dataset {
    /// Parses and validates a version 1 dataset. Benchmark-specific scope
    /// checks are applied separately to the committed benchmark.
    ///
    /// # Errors
    ///
    /// Returns the first JSON or format error without including input text.
    pub fn parse(input: &str) -> Result<Self, DatasetError> {
        let dataset: Self = serde_json::from_str(input).map_err(|error| DatasetError::Json {
            line: error.line(),
            column: error.column(),
        })?;
        dataset.validate()?;
        Ok(dataset)
    }

    /// The dataset's stable identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The content version of this dataset.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Relevance grades declared by the dataset.
    #[must_use]
    pub const fn grades(&self) -> &BTreeMap<String, String> {
        &self.grades
    }

    /// Canonical page records in dataset order.
    #[must_use]
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// Queries in dataset order.
    #[must_use]
    pub fn queries(&self) -> &[Query] {
        &self.queries
    }

    fn validate(&self) -> Result<(), DatasetError> {
        self.validate_metadata()?;
        for text in self.free_text_fields() {
            if text.chars().count() > MAX_FIELD_CHARS {
                return Err(DatasetError::TextTooLong);
            }
        }
        let pages = self.validate_pages()?;
        let (used_pages, query_ids) = self.validate_queries(&pages)?;
        if used_pages.len() != pages.len() {
            return Err(DatasetError::Metadata("recorded pages must be judged"));
        }
        self.validate_retired_ids(&query_ids)
    }

    fn validate_metadata(&self) -> Result<(), DatasetError> {
        if self.format_version != 1 {
            return Err(DatasetError::Metadata("unsupported format version"));
        }
        for (value, field) in [
            (&self.id, "dataset_id"),
            (&self.version, "dataset_version"),
            (&self.source.method, "method"),
            (&self.source.licence_note, "licence_note"),
        ] {
            if value.trim().is_empty() {
                return Err(DatasetError::Metadata(field));
            }
        }
        if !self.source.site.starts_with("https://") {
            return Err(DatasetError::Metadata("source must start with https://"));
        }
        if !is_dated(&self.created) || !is_dated(&self.source.observed_on) {
            return Err(DatasetError::Metadata("dates must use YYYY-MM-DD"));
        }
        Ok(())
    }

    fn validate_pages(&self) -> Result<HashMap<&str, &Page>, DatasetError> {
        let mut pages = HashMap::new();
        for (index, page) in self.pages.iter().enumerate() {
            if pages.insert(page.title.as_str(), page).is_some() {
                return Err(DatasetError::Page {
                    index: index + 1,
                    rule: "duplicate title",
                });
            }
            page.validate().map_err(|rule| DatasetError::Page {
                index: index + 1,
                rule,
            })?;
        }
        Ok(pages)
    }

    fn validate_queries<'a>(
        &'a self,
        pages: &HashMap<&str, &Page>,
    ) -> Result<(HashSet<&'a str>, HashSet<&'a str>), DatasetError> {
        let mut ids = HashSet::new();
        let mut texts = HashSet::new();
        let mut used_pages = HashSet::new();
        for (index, query) in self.queries.iter().enumerate() {
            let error = |rule| DatasetError::Query {
                index: index + 1,
                rule,
            };
            query.validate().map_err(error)?;
            if !ids.insert(query.id.as_str()) {
                return Err(error("duplicate id"));
            }
            if !texts.insert(query.text.to_lowercase()) {
                return Err(error("duplicate query text"));
            }
            if !self.modes.contains_key(&query.mode) {
                return Err(error("undeclared mode"));
            }
            for judgement in &query.judgements {
                self.validate_judgement(judgement, pages).map_err(error)?;
                used_pages.insert(judgement.page.as_str());
            }
        }
        Ok((used_pages, ids))
    }

    fn validate_judgement(
        &self,
        judgement: &Judgement,
        pages: &HashMap<&str, &Page>,
    ) -> Result<(), &'static str> {
        let Some(recorded) = pages.get(judgement.page.as_str()) else {
            return Err("judgement names an unrecorded page");
        };
        if judgement
            .section
            .as_ref()
            .is_some_and(|section| !recorded.observed_sections.contains(section))
        {
            return Err("judgement names an unobserved section");
        }
        if !self.grades.contains_key(&judgement.grade.to_string()) {
            return Err("undeclared grade");
        }
        Ok(())
    }

    fn validate_retired_ids(&self, query_ids: &HashSet<&str>) -> Result<(), DatasetError> {
        for retired in &self.retired_query_ids {
            if retired.reason.trim().is_empty() {
                return Err(DatasetError::Metadata("retired ids require reasons"));
            }
            if query_ids.contains(retired.id.as_str()) {
                return Err(DatasetError::Metadata("a query uses a retired id"));
            }
        }
        Ok(())
    }

    fn free_text_fields(&self) -> Vec<&str> {
        let mut fields: Vec<&str> = vec![
            &self.id,
            &self.version,
            &self.created,
            &self.source.site,
            &self.source.observed_on,
            &self.source.method,
            &self.source.licence_note,
        ];
        fields.extend(self.grades.values().map(String::as_str));
        fields.extend(self.modes.values().map(String::as_str));
        for retired in &self.retired_query_ids {
            fields.push(&retired.id);
            fields.push(&retired.reason);
        }
        for page in &self.pages {
            fields.push(&page.title);
            fields.extend(page.requested_as.iter().map(String::as_str));
            fields.extend(page.last_modified.as_deref());
            fields.extend(page.observed_sections.iter().map(String::as_str));
            fields.extend(page.note.as_deref());
        }
        for query in &self.queries {
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
}

impl Page {
    fn validate(&self) -> Result<(), &'static str> {
        if self.observed_sections.is_empty() {
            return Err("missing section list");
        }
        let unique: HashSet<_> = self.observed_sections.iter().collect();
        if unique.len() != self.observed_sections.len() {
            return Err("duplicate observed section");
        }
        if self.oldid.is_some() != self.last_modified.is_some()
            || self.oldid.is_none() != (self.revision_basis == RevisionBasis::NotObserved)
        {
            return Err("inconsistent revision fields");
        }
        if self
            .last_modified
            .as_deref()
            .is_some_and(|date| !is_dated(date))
        {
            return Err("revision date must use YYYY-MM-DD");
        }
        if !self.headings_complete && !has_text(self.note.as_deref()) {
            return Err("incomplete heading list requires a note");
        }
        Ok(())
    }
}

impl Query {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.id.strip_prefix('q').is_some_and(|digits| {
            digits.len() == 3 && digits.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            return Err("id must use qNNN");
        }
        if self.text.trim().is_empty() || self.intent.trim().is_empty() {
            return Err("query text and intent must be nonempty");
        }
        if self.mode == "hardcore_ironman" && !self.tags.iter().any(|tag| tag == "disambiguation") {
            return Err("Hardcore Ironman mode requires the disambiguation tag");
        }
        let scorable = self.relevance.is_scorable();
        if scorable && !self.judgements.iter().any(|judgement| judgement.grade == 2) {
            return Err("scorable query has no essential judgement");
        }
        if scorable && self.references == References::Unverified {
            return Err("scorable query has unverified references");
        }
        if !(scorable && self.references == References::Verified || has_text(self.notes.as_deref()))
        {
            return Err("query needs notes explaining its uncertainty");
        }
        if self.judgements_overlap() {
            return Err("overlapping judgements");
        }
        Ok(())
    }

    fn judgements_overlap(&self) -> bool {
        self.judgements.iter().enumerate().any(|(index, a)| {
            self.judgements[index + 1..].iter().any(|b| {
                a.page == b.page
                    && (covers(a.section.as_deref(), b.section.as_deref())
                        || covers(b.section.as_deref(), a.section.as_deref()))
            })
        })
    }
}

/// Validates the documented date shape, without calendar validation.
fn is_dated(text: &str) -> bool {
    text.len() == 10
        && text.char_indices().all(|(index, character)| match index {
            4 | 7 => character == '-',
            _ => character.is_ascii_digit(),
        })
}

fn has_text(text: Option<&str>) -> bool {
    text.is_some_and(|value| !value.trim().is_empty())
}

/// Whether the outer path covers the inner path, including whole-page paths.
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

// Dataset text is useful only when deliberately read, not in diagnostics.
macro_rules! redacted_debug {
    ($($record:ty),+ $(,)?) => {
        $(
            impl fmt::Debug for $record {
                fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.debug_struct(stringify!($record)).finish_non_exhaustive()
                }
            }
        )+
    };
}

redacted_debug!(Dataset, Page, Query, Judgement);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> Value {
        json!({
            "format_version": 1,
            "dataset_id": "synthetic-ledgers",
            "dataset_version": "1.0.0",
            "created": "2026-01-01",
            "source": {
                "site": "https://ledger.example",
                "observed_on": "2026-01-02",
                "method": "synthetic example",
                "licence_note": "original fixture"
            },
            "modes": {"mode_independent": "Applies to all ledgers"},
            "grades": {"1": "Supporting", "2": "Essential", "3": "Extra"},
            "pages": [{
                "title": "Ledger",
                "requested_as": ["The Ledger"],
                "oldid": 42,
                "last_modified": "2026-01-01",
                "revision_basis": "footer",
                "headings_complete": true,
                "observed_sections": ["Ink", "Ink > Colour"],
                "note": null
            }],
            "queries": [{
                "id": "q001",
                "category": "synthetic",
                "mode": "mode_independent",
                "query": "Which ink is used?",
                "intent": "Find the material",
                "references": "verified",
                "relevance": "reviewed",
                "tags": [],
                "judgements": [{"page": "Ledger", "section": "Ink", "grade": 2}],
                "notes": null
            }],
            "retired_query_ids": [{"id": "q005", "reason": "Superseded"}]
        })
    }

    fn parse(value: &Value) -> Result<Dataset, DatasetError> {
        Dataset::parse(&value.to_string())
    }

    #[test]
    fn accepts_a_dataset_with_different_scope() {
        let dataset = parse(&fixture()).unwrap();

        assert_eq!(dataset.id(), "synthetic-ledgers");
        assert_eq!(dataset.version(), "1.0.0");
        assert_eq!(dataset.grades().len(), 3);
        assert_eq!(dataset.queries().len(), 1);
        assert_eq!(dataset.pages()[0].oldid, Some(42));
        assert_eq!(
            dataset.queries()[0].judgements[0].section.as_deref(),
            Some("Ink")
        );
    }

    #[test]
    fn json_and_debug_diagnostics_omit_input_text() {
        let mut data = fixture();
        data["queries"][0]["query"] = json!("SyntheticMarker");
        let dataset = parse(&data).unwrap();
        assert!(
            !format!(
                "{dataset:?} {:?} {:?} {:?}",
                dataset.queries(),
                dataset.pages(),
                dataset.queries()[0].judgements
            )
            .contains("SyntheticMarker")
        );

        data["SyntheticMarker"] = json!(true);
        let error = parse(&data).unwrap_err();
        assert!(matches!(error, DatasetError::Json { .. }));
        assert!(!format!("{error:?} {error}").contains("SyntheticMarker"));
    }

    #[test]
    fn invalid_metadata_is_rejected() {
        for (field, value, reason) in [
            ("/format_version", json!(2), "unsupported format version"),
            ("/dataset_id", json!(" "), "dataset_id"),
            ("/dataset_version", json!(" "), "dataset_version"),
            (
                "/source/site",
                json!("file:///ledger"),
                "source must start with https://",
            ),
            ("/source/method", json!(""), "method"),
            ("/source/licence_note", json!(""), "licence_note"),
            ("/created", json!("2026/01/01"), "dates must use YYYY-MM-DD"),
            (
                "/source/observed_on",
                json!("2026-01"),
                "dates must use YYYY-MM-DD",
            ),
            (
                "/retired_query_ids/0/reason",
                json!(""),
                "retired ids require reasons",
            ),
            (
                "/retired_query_ids/0/id",
                json!("q001"),
                "a query uses a retired id",
            ),
        ] {
            let mut data = fixture();
            *data.pointer_mut(field).unwrap() = value;
            assert_eq!(
                parse(&data).unwrap_err(),
                DatasetError::Metadata(reason),
                "{field}"
            );
        }
    }

    #[test]
    fn invalid_page_records_are_rejected() {
        for (field, value, rule) in [
            ("observed_sections", json!([]), "missing section list"),
            (
                "observed_sections",
                json!(["Ink", "Ink"]),
                "duplicate observed section",
            ),
            ("oldid", Value::Null, "inconsistent revision fields"),
            (
                "revision_basis",
                json!("not_observed"),
                "inconsistent revision fields",
            ),
            (
                "last_modified",
                json!("2026/01/01"),
                "revision date must use YYYY-MM-DD",
            ),
            (
                "headings_complete",
                json!(false),
                "incomplete heading list requires a note",
            ),
        ] {
            let mut data = fixture();
            data["pages"][0][field] = value;
            assert_eq!(
                parse(&data).unwrap_err(),
                DatasetError::Page { index: 1, rule },
                "{field}"
            );
        }
    }

    #[test]
    fn invalid_query_records_are_rejected() {
        for (field, value, rule) in [
            ("/id", json!("x1"), "id must use qNNN"),
            (
                "/query",
                json!(" "),
                "query text and intent must be nonempty",
            ),
            (
                "/intent",
                json!(" "),
                "query text and intent must be nonempty",
            ),
            ("/mode", json!("another_mode"), "undeclared mode"),
            (
                "/mode",
                json!("hardcore_ironman"),
                "Hardcore Ironman mode requires the disambiguation tag",
            ),
            (
                "/references",
                json!("unverified"),
                "scorable query has unverified references",
            ),
            (
                "/references",
                json!("partial"),
                "query needs notes explaining its uncertainty",
            ),
            (
                "/judgements/0/grade",
                json!(1),
                "scorable query has no essential judgement",
            ),
            (
                "/judgements/0/page",
                json!("Other"),
                "judgement names an unrecorded page",
            ),
            (
                "/judgements/0/section",
                json!("Paper"),
                "judgement names an unobserved section",
            ),
        ] {
            let mut data = fixture();
            *data["queries"][0].pointer_mut(field).unwrap() = value;
            assert_eq!(
                parse(&data).unwrap_err(),
                DatasetError::Query { index: 1, rule },
                "{field}"
            );
        }
    }

    #[test]
    fn distinct_records_are_required() {
        let mut data = fixture();
        let page = data["pages"][0].clone();
        data["pages"].as_array_mut().unwrap().push(page);
        assert_eq!(
            parse(&data).unwrap_err(),
            DatasetError::Page {
                index: 2,
                rule: "duplicate title"
            },
        );

        let mut data = fixture();
        let query = data["queries"][0].clone();
        data["queries"].as_array_mut().unwrap().push(query);
        assert_eq!(
            parse(&data).unwrap_err(),
            DatasetError::Query {
                index: 2,
                rule: "duplicate id"
            },
        );

        data["queries"][1]["id"] = json!("q002");
        data["queries"][1]["query"] = json!("WHICH INK IS USED?");
        assert_eq!(
            parse(&data).unwrap_err(),
            DatasetError::Query {
                index: 2,
                rule: "duplicate query text"
            },
        );
    }

    #[test]
    fn judgement_sections_cannot_overlap_ancestors_or_page_hits() {
        for section in [Value::Null, json!("Ink"), json!("Ink > Colour")] {
            let mut data = fixture();
            data["queries"][0]["judgements"]
                .as_array_mut()
                .unwrap()
                .push(json!({"page": "Ledger", "section": section, "grade": 2}));
            assert_eq!(
                parse(&data).unwrap_err(),
                DatasetError::Query {
                    index: 1,
                    rule: "overlapping judgements"
                },
            );
        }
    }

    #[test]
    fn every_judgement_grade_must_be_declared() {
        let mut data = fixture();
        data["queries"][0]["judgements"][0]["grade"] = json!(9);
        data["queries"][0]["relevance"] = json!("ambiguous");
        data["queries"][0]["notes"] = json!("Insufficient independent evidence");
        assert_eq!(
            parse(&data).unwrap_err(),
            DatasetError::Query {
                index: 1,
                rule: "undeclared grade"
            },
        );
    }

    #[test]
    fn every_recorded_page_must_be_judged() {
        let mut data = fixture();
        let mut page = data["pages"][0].clone();
        page["title"] = json!("Unjudged ledger");
        data["pages"].as_array_mut().unwrap().push(page);
        assert_eq!(
            parse(&data).unwrap_err(),
            DatasetError::Metadata("recorded pages must be judged"),
        );
    }

    #[test]
    fn text_limit_counts_characters_not_bytes() {
        let mut data = fixture();
        data["queries"][0]["query"] = json!("é".repeat(MAX_FIELD_CHARS));
        assert!(parse(&data).is_ok());
        data["queries"][0]["query"] = json!("é".repeat(MAX_FIELD_CHARS + 1));
        assert_eq!(parse(&data).unwrap_err(), DatasetError::TextTooLong);
    }
}
