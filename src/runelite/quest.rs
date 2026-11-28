//! Parsing of Character State Exporter's quest observations.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use super::ExportedAt;
use super::envelope::{EnvelopeError, Fields, decode_object, read_envelope, required};

/// Largest quest export accepted, in bytes.
pub const MAX_QUEST_EXPORT_BYTES: usize = 1024 * 1024;
/// Largest number of rows accepted in a quest export.
pub const MAX_QUEST_ROWS: usize = 1_024;

const MAX_QUEST_NAME_BYTES: usize = 256;

/// A quest identifier from the `RuneLite` client, with its own namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuneLiteQuestId(pub u32);

/// State recorded by the exporter for a quest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestState {
    /// The export records this quest as finished.
    Finished,
    /// The export records this quest as in progress.
    InProgress,
    /// The export records this quest as not started.
    NotStarted,
}

/// One quest observation from the export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestRecord {
    /// Identifier in the `RuneLite` namespace.
    pub runelite_id: RuneLiteQuestId,
    /// Untrusted display text as exported.
    pub display_name: String,
    /// Recorded quest state.
    pub state: QuestState,
}

/// Quest progress observed in one export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestProgress {
    exported_at: ExportedAt,
    plugin_version: Option<String>,
    quests: Vec<QuestRecord>,
    warnings: Vec<QuestWarning>,
}

impl QuestProgress {
    /// The export's timestamp of last payload change.
    #[must_use]
    pub const fn exported_at(&self) -> &ExportedAt {
        &self.exported_at
    }

    /// Optional untrusted plugin version text.
    #[must_use]
    pub fn plugin_version(&self) -> Option<&str> {
        self.plugin_version.as_deref()
    }

    /// Observed quests in file order. Omitted quests are unrecorded.
    #[must_use]
    pub fn quests(&self) -> &[QuestRecord] {
        &self.quests
    }

    /// Non-fatal findings about the export.
    #[must_use]
    pub fn warnings(&self) -> &[QuestWarning] {
        &self.warnings
    }
}

/// A non-fatal finding that leaves quest observations unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestWarning {
    /// A recorded summary count disagrees with the corresponding row count.
    SummaryMismatch,
    /// Every recorded quest is not started. This can reflect a new account
    /// or incomplete client initialization.
    AllNotStarted,
}

/// Why a quest export could not be parsed. Variants carry structural details
/// only, such as static field names and row positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum QuestParseError {
    /// The input exceeds [`MAX_QUEST_EXPORT_BYTES`].
    #[error("quest export exceeds the byte limit")]
    InputTooLarge,
    /// The input ended prematurely. It may have been read during a write.
    #[error("export ended unexpectedly (a write may be in progress)")]
    Truncated,
    /// The input is not valid JSON.
    #[error("export is not valid JSON")]
    InvalidJson,
    /// The input is valid JSON with a non-object top level.
    #[error("quest export must be an object")]
    UnexpectedShape,
    /// A required top-level field is absent.
    #[error("required field `{field}` is missing")]
    MissingField {
        /// The field name.
        field: &'static str,
    },
    /// A top-level field has the wrong type or is outside its bounds.
    #[error("field `{field}` has an invalid value")]
    InvalidField {
        /// The field name.
        field: &'static str,
    },
    /// There are more than [`MAX_QUEST_ROWS`] rows.
    #[error("quest export exceeds the row limit")]
    TooManyRows,
    /// A quest row is not an object.
    #[error("quest row {row} must be an object")]
    InvalidRow {
        /// One-based row position.
        row: usize,
    },
    /// A quest row field is missing, has the wrong type or is out of range.
    #[error("quest row {row} field `{field}` is missing or invalid")]
    InvalidRowField {
        /// One-based row position.
        row: usize,
        /// The field name.
        field: &'static str,
    },
    /// The state string is not a recognized exporter state.
    #[error("quest row {row} has an unrecognized state")]
    UnknownState {
        /// One-based row position.
        row: usize,
    },
    /// An ID already appeared in an earlier row.
    #[error("quest row {row} repeats an earlier ID")]
    DuplicateId {
        /// One-based row position of the repeat.
        row: usize,
    },
}

impl QuestParseError {
    /// Whether retrying shortly may succeed if the export was being written.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

/// Parses one `quests.json` export, without file or network access.
///
/// Display names may be at most 256 UTF-8 bytes. The timestamp and optional
/// plugin version may each be at most 128 UTF-8 bytes.
///
/// # Errors
///
/// Returns [`QuestParseError`] for invalid, missing or oversized input. A
/// summary disagreement or nonempty all-not-started snapshot is reported as
/// a warning.
pub fn parse_quest_export(bytes: &[u8]) -> Result<QuestProgress, QuestParseError> {
    let fields = decode_object(bytes, MAX_QUEST_EXPORT_BYTES)?;
    let envelope = read_envelope(&fields)?;
    let quests = read_quest_list(&fields)?;
    let summary = fields.get("summary").map(read_summary).transpose()?;
    let warnings = warnings_for(&quests, summary.as_ref());
    Ok(QuestProgress {
        exported_at: envelope.exported_at,
        plugin_version: envelope.plugin_version,
        quests,
        warnings,
    })
}

impl From<EnvelopeError> for QuestParseError {
    fn from(error: EnvelopeError) -> Self {
        match error {
            EnvelopeError::InputTooLarge => Self::InputTooLarge,
            EnvelopeError::Truncated => Self::Truncated,
            EnvelopeError::InvalidJson => Self::InvalidJson,
            EnvelopeError::UnexpectedShape => Self::UnexpectedShape,
            EnvelopeError::MissingField { field } => Self::MissingField { field },
            EnvelopeError::InvalidField { field } => Self::InvalidField { field },
        }
    }
}

fn read_quest_list(fields: &Fields) -> Result<Vec<QuestRecord>, QuestParseError> {
    let rows = required(fields, "quests")?
        .as_array()
        .ok_or(QuestParseError::InvalidField { field: "quests" })?;
    read_quests(rows)
}

fn read_quests(rows: &[Value]) -> Result<Vec<QuestRecord>, QuestParseError> {
    if rows.len() > MAX_QUEST_ROWS {
        return Err(QuestParseError::TooManyRows);
    }
    let mut seen = HashSet::with_capacity(rows.len());
    let mut quests = Vec::with_capacity(rows.len());
    for (index, value) in rows.iter().enumerate() {
        let row = index + 1;
        let quest = read_quest(value, row)?;
        if !seen.insert(quest.runelite_id) {
            return Err(QuestParseError::DuplicateId { row });
        }
        quests.push(quest);
    }
    Ok(quests)
}

fn read_quest(value: &Value, row: usize) -> Result<QuestRecord, QuestParseError> {
    let fields = value
        .as_object()
        .ok_or(QuestParseError::InvalidRow { row })?;
    let invalid = |field| QuestParseError::InvalidRowField { row, field };
    let id = fields
        .get("id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| invalid("id"))?;
    let name = fields
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| name.len() <= MAX_QUEST_NAME_BYTES)
        .ok_or_else(|| invalid("name"))?;
    let state = match fields.get("state").and_then(Value::as_str) {
        Some("FINISHED") => QuestState::Finished,
        Some("IN_PROGRESS") => QuestState::InProgress,
        Some("NOT_STARTED") => QuestState::NotStarted,
        Some(_) => return Err(QuestParseError::UnknownState { row }),
        None => return Err(invalid("state")),
    };
    Ok(QuestRecord {
        runelite_id: RuneLiteQuestId(id),
        display_name: name.to_owned(),
        state,
    })
}

#[derive(Default, Deserialize, PartialEq, Eq)]
struct QuestCounts {
    finished: u64,
    in_progress: u64,
    not_started: u64,
}

fn read_summary(value: &Value) -> Result<QuestCounts, QuestParseError> {
    let invalid = QuestParseError::InvalidField { field: "summary" };
    if !value.is_object() {
        return Err(invalid);
    }
    QuestCounts::deserialize(value).map_err(|_| invalid)
}

fn warnings_for(quests: &[QuestRecord], summary: Option<&QuestCounts>) -> Vec<QuestWarning> {
    let mut actual = QuestCounts::default();
    for quest in quests {
        match quest.state {
            QuestState::Finished => actual.finished += 1,
            QuestState::InProgress => actual.in_progress += 1,
            QuestState::NotStarted => actual.not_started += 1,
        }
    }
    let mut warnings = Vec::new();
    if summary.is_some_and(|reported| reported != &actual) {
        warnings.push(QuestWarning::SummaryMismatch);
    }
    if actual.not_started > 0 && actual.finished == 0 && actual.in_progress == 0 {
        warnings.push(QuestWarning::AllNotStarted);
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const QUESTS: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/quests.synthetic.json");

    fn export() -> Value {
        serde_json::from_slice(QUESTS).unwrap()
    }

    fn parse(input: &Value) -> Result<QuestProgress, QuestParseError> {
        parse_quest_export(input.to_string().as_bytes())
    }

    #[test]
    fn preserves_export_envelope_order_and_distinct_states() {
        let progress = parse_quest_export(QUESTS).unwrap();
        assert_eq!(
            progress.exported_at().text(),
            "2026-02-01T11:12:13.123456-02:00"
        );
        assert_eq!(
            progress.exported_at().timestamp().subsec_nanosecond(),
            123_456_000
        );
        assert_eq!(progress.plugin_version(), Some("0.6.0"));
        assert_eq!(
            progress
                .quests()
                .iter()
                .map(|row| (row.runelite_id, row.state))
                .collect::<Vec<_>>(),
            [
                (RuneLiteQuestId(203), QuestState::Finished),
                (RuneLiteQuestId(18), QuestState::InProgress),
                (RuneLiteQuestId(90), QuestState::NotStarted),
            ]
        );
        assert_eq!(progress.quests()[0].display_name, "Synthetic quest A");
        assert_eq!(progress.warnings(), []);
    }

    #[test]
    fn omitted_quest_list_differs_from_present_empty_list() {
        let mut input = export();
        let fields = input.as_object_mut().unwrap();
        fields.remove("summary");
        fields.remove("plugin_version");
        fields.remove("quests");
        assert_eq!(
            parse(&input),
            Err(QuestParseError::MissingField { field: "quests" })
        );
        input["quests"] = Value::Null;
        assert_eq!(
            parse(&input),
            Err(QuestParseError::InvalidField { field: "quests" })
        );
        input["quests"] = json!([]);
        let progress = parse(&input).unwrap();
        assert_eq!(progress.quests(), []);
        assert_eq!(progress.warnings(), []);
        assert_eq!(progress.plugin_version(), None);
    }

    #[test]
    fn nonempty_all_not_started_is_valid_with_an_advisory_warning() {
        let mut input = export();
        input["quests"][0]["state"] = json!("NOT_STARTED");
        input["quests"][1]["state"] = json!("NOT_STARTED");
        let progress = parse(&input).unwrap();
        assert_eq!(
            progress.warnings(),
            [QuestWarning::SummaryMismatch, QuestWarning::AllNotStarted]
        );
        assert!(
            progress
                .quests()
                .iter()
                .all(|row| row.state == QuestState::NotStarted)
        );
        input["summary"] = json!({"finished": 0, "in_progress": 0, "not_started": 3});
        assert_eq!(
            parse(&input).unwrap().warnings(),
            [QuestWarning::AllNotStarted]
        );
    }

    #[test]
    fn unknown_states_and_duplicate_ids_are_sanitized_errors() {
        let mut input = export();
        input["quests"][1]["id"] = json!(203);
        assert_eq!(parse(&input), Err(QuestParseError::DuplicateId { row: 2 }));
        input["quests"][1]["id"] = json!(18);
        input["quests"][1]["state"] = json!("private-progress");
        let error = parse(&input).unwrap_err();
        assert_eq!(error, QuestParseError::UnknownState { row: 2 });
        assert!(!format!("{error} {error:?}").contains("private-progress"));
    }

    #[test]
    fn differentiates_truncation_from_invalid_json_or_shape() {
        let error = parse_quest_export(b"{\"quests\": [").unwrap_err();
        assert_eq!(error, QuestParseError::Truncated);
        assert!(error.is_transient());
        assert_eq!(
            parse_quest_export(b"{?}"),
            Err(QuestParseError::InvalidJson)
        );
        assert_eq!(
            parse_quest_export(b"[]"),
            Err(QuestParseError::UnexpectedShape)
        );
    }

    #[test]
    fn enforces_input_row_and_utf8_bounds() {
        assert_eq!(
            parse_quest_export(&vec![b'!'; MAX_QUEST_EXPORT_BYTES + 1]),
            Err(QuestParseError::InputTooLarge)
        );
        let mut input = export();
        input["quests"][0]["name"] = json!("é".repeat(128));
        assert!(parse(&input).is_ok());
        input["quests"][0]["name"] = json!("é".repeat(129));
        assert_eq!(
            parse(&input),
            Err(QuestParseError::InvalidRowField {
                row: 1,
                field: "name"
            })
        );
        input["quests"] = json!(vec![Value::Null; MAX_QUEST_ROWS + 1]);
        assert_eq!(parse(&input), Err(QuestParseError::TooManyRows));
    }

    #[test]
    fn validates_present_summary_and_row_shape() {
        let mut input = export();
        input["summary"]["finished"] = json!(-1);
        assert_eq!(
            parse(&input),
            Err(QuestParseError::InvalidField { field: "summary" })
        );
        input.as_object_mut().unwrap().remove("summary");
        input["quests"][1] = json!([]);
        assert_eq!(parse(&input), Err(QuestParseError::InvalidRow { row: 2 }));
    }

    #[test]
    fn the_envelope_timestamp_and_plugin_version_are_validated() {
        let mut input = export();
        let plugin_version =
            |input: &Value| parse(input).map(|parsed| parsed.plugin_version().map(str::len));
        let error = |input: &Value| parse(input).err();

        input["plugin_version"] = json!("v".repeat(128));
        assert_eq!(plugin_version(&input), Ok(Some(128)));
        for invalid in [json!("v".repeat(129)), json!(7)] {
            input["plugin_version"] = invalid;
            assert_eq!(
                error(&input),
                Some(QuestParseError::InvalidField {
                    field: "plugin_version"
                })
            );
        }
        input["plugin_version"] = Value::Null;
        assert_eq!(plugin_version(&input), Ok(None));
        input.as_object_mut().unwrap().remove("plugin_version");
        assert_eq!(plugin_version(&input), Ok(None));

        // With both invalid, `exported_at` is the one reported.
        input["exported_at"] = json!("yesterday");
        input["plugin_version"] = json!(7);
        assert_eq!(
            error(&input),
            Some(QuestParseError::InvalidField {
                field: "exported_at"
            })
        );
        input.as_object_mut().unwrap().remove("exported_at");
        assert_eq!(
            error(&input),
            Some(QuestParseError::MissingField {
                field: "exported_at"
            })
        );
    }
}
