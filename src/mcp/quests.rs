//! The `quest_status` tool: quest states from the local Character Exporter
//! file.
//!
//! This module only adapts. Reading is [`LocalExportSource`](crate::runelite::LocalExportSource)
//! and parsing is [`parse_quest_export`](crate::runelite::parse_quest_export);
//! here a parsed export becomes a filtered, paged response and a read failure
//! becomes a fixed, safe status. Quest IDs are `RuneLite`'s own and are not
//! Wiki or game-cache IDs; nothing here maps names to Wiki pages, checks
//! requirements or says which quests are available. A quest absent from the
//! file is unrecorded, never "not started".

use jiff::Timestamp;
use rmcp::{
    handler::server::{tool::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, ErrorData},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};

use super::OsrsMcpServer;
use super::containers::ContentsStatus;
use super::export_output::{
    ExportFailure, ObservationTime, Page, bounded, bounded_name_to, failure_for, into_result,
};
use crate::runelite::{
    ExportReadError, QUEST_FILE_NAME, QuestProgress, QuestRecord, QuestState, QuestWarning,
};

/// Quests returned when `limit` is not given.
const DEFAULT_LIMIT: usize = 100;
/// Most quests one call can return.
const MAX_LIMIT: usize = 250;
/// Longest quest name returned, in characters. Names come from the file.
const MAX_QUEST_NAME_CHARS: usize = 64;
/// Most warnings returned.
const MAX_WARNINGS: usize = 8;

/// A quest state as exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum QuestStateOutput {
    /// The export records the quest as finished.
    Finished,
    /// The export records the quest as in progress.
    InProgress,
    /// The export records the quest as not started.
    NotStarted,
}

impl From<QuestState> for QuestStateOutput {
    fn from(state: QuestState) -> Self {
        match state {
            QuestState::Finished => Self::Finished,
            QuestState::InProgress => Self::InProgress,
            QuestState::NotStarted => Self::NotStarted,
        }
    }
}

/// Arguments of the `quest_status` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct QuestStatusInput {
    /// Only quests the export records in this state. The counts in the response
    /// ignore this filter. Omit to list quests in every state.
    state: Option<QuestStateOutput>,
    /// Matching quests to skip, in the order the file lists them. Defaults to 0.
    offset: Option<u32>,
    /// Most quests to return, 1 to 250. Defaults to 100.
    limit: Option<u32>,
}

/// A validated request: which quests, then which page of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Query {
    state: Option<QuestStateOutput>,
    page: Page,
}

impl QuestStatusInput {
    fn into_query(self) -> Result<Query, String> {
        let page = Page::from_arguments(self.offset, self.limit, DEFAULT_LIMIT, MAX_LIMIT)?;
        Ok(Query {
            state: self.state,
            page,
        })
    }
}

/// Quests by recorded state across the whole export, whatever the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct QuestCounts {
    /// Quests present in the export. A quest absent from it is unrecorded and
    /// is not counted anywhere.
    pub recorded: usize,
    /// Quests recorded as finished.
    pub finished: usize,
    /// Quests recorded as in progress.
    pub in_progress: usize,
    /// Quests recorded as not started.
    pub not_started: usize,
}

/// One recorded quest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct QuestOutput {
    /// `RuneLite`'s internal quest ID. It is not a Wiki or game-cache ID.
    pub runelite_id: u32,
    /// `RuneLite`'s display name, which can differ from the Wiki title.
    /// Untrusted text from the file, cut to 64 characters.
    pub name: String,
    /// True if `name` was cut short and is therefore not the full name.
    pub name_truncated: bool,
    /// The recorded state.
    pub state: QuestStateOutput,
}

/// A non-fatal finding from the import. It never changes the quests listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestWarningOutput {
    /// The file's own summary counters disagree with its quest rows.
    SummaryMismatch,
    /// Every recorded quest is not started. That can be a new account or a
    /// snapshot taken before the client finished loading; it is advisory only.
    AllNotStarted,
}

/// Successful result of `quest_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct QuestStatus {
    /// Always `ok`.
    pub status: ContentsStatus,
    /// Timing of this file only. Other files have their own timestamps and
    /// are not guaranteed to be consistent with this one.
    #[serde(flatten)]
    pub observed: ObservationTime,
    /// Counts over every recorded quest, ignoring `state`, `offset` and `limit`.
    pub counts: QuestCounts,
    /// The requested page of matching quests, in the order the file lists them.
    pub quests: Vec<QuestOutput>,
    /// Quests matching the `state` filter (all recorded quests without one),
    /// counted before paging.
    pub matched: usize,
    /// Matching quests skipped before this page.
    pub offset: usize,
    /// The page size applied.
    pub limit: usize,
    /// True if matching quests after this page exist.
    pub truncated: bool,
    /// Non-fatal findings.
    pub warnings: Vec<QuestWarningOutput>,
}

#[tool_router(router = quests_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Reports quest states from the local Character Exporter file.
    #[tool(
        name = "quest_status",
        description = "Read the player's quest states (finished, in_progress, not_started) from \
                       the local RuneLite Character Exporter file configured for this server, \
                       with the exporter timestamp and import warnings. Reads the file on \
                       every call. Optionally filter by state; offset and limit (default 100, \
                       at most 250) page the matching quests in file order. counts always \
                       covers every recorded quest and ignores the filter; matched counts \
                       quests after filtering and before paging. runelite_id is RuneLite's own \
                       internal quest id, not a Wiki or game-cache id, and names can differ \
                       from Wiki titles. A quest absent from the list is unrecorded, not \
                       not started. The timestamp is the last change to the quest file, not \
                       the time of reading, and the file is not consistent with any other \
                       export file. A missing file is reported as file_not_found, never as an \
                       empty list. Does not check requirements or say which quests are \
                       available. Failures report a status such as not_configured or \
                       file_not_found.",
        annotations(
            title = "Quest status",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        output_schema = schema_for_output::<QuestStatus>()
    )]
    async fn quest_status(
        &self,
        Parameters(input): Parameters<QuestStatusInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = input
            .into_query()
            .map_err(|message| ErrorData::invalid_params(message, None))?;

        // The read is blocking file I/O, so it stays off the async executor.
        let source = self.export_source.clone();
        let read = tokio::task::spawn_blocking(move || source.read_quests())
            .await
            .map_err(|_| ErrorData::internal_error("the export read task failed", None))?;

        // One clock sample, taken after the read, serves both `read_at` and the age.
        let outcome = read
            .map(|progress| quest_status(&progress, &query, Timestamp::now()))
            .map_err(|error| quest_failure(&error));
        into_result(outcome)
    }
}

/// The fixed failure for `quests.json`.
fn quest_failure(error: &ExportReadError) -> ExportFailure {
    failure_for(error, QUEST_FILE_NAME)
}

/// Builds the response for a parsed export, read at `read_at`.
fn quest_status(progress: &QuestProgress, query: &Query, read_at: Timestamp) -> QuestStatus {
    let quests = progress.quests();
    // Filtering looks at every quest; the page is taken afterwards.
    let matching: Vec<&QuestRecord> = quests
        .iter()
        .filter(|quest| {
            query
                .state
                .is_none_or(|state| QuestStateOutput::from(quest.state) == state)
        })
        .collect();
    let (shown, truncated) = query.page.select(&matching, |quest| quest_output(quest));
    let (warnings, _) = bounded(progress.warnings(), MAX_WARNINGS, |warning| {
        warning_output(*warning)
    });

    QuestStatus {
        status: ContentsStatus::Ok,
        observed: ObservationTime::new(progress.exported_at(), progress.plugin_version(), read_at),
        counts: counts(quests),
        truncated,
        quests: shown,
        matched: matching.len(),
        offset: query.page.offset,
        limit: query.page.limit,
        warnings,
    }
}

fn counts(quests: &[QuestRecord]) -> QuestCounts {
    let in_state = |wanted| quests.iter().filter(|quest| quest.state == wanted).count();
    QuestCounts {
        recorded: quests.len(),
        finished: in_state(QuestState::Finished),
        in_progress: in_state(QuestState::InProgress),
        not_started: in_state(QuestState::NotStarted),
    }
}

fn quest_output(quest: &QuestRecord) -> QuestOutput {
    let (name, name_truncated) = bounded_name_to(&quest.display_name, MAX_QUEST_NAME_CHARS);
    QuestOutput {
        runelite_id: quest.runelite_id.0,
        name,
        name_truncated,
        state: quest.state.into(),
    }
}

const fn warning_output(warning: QuestWarning) -> QuestWarningOutput {
    match warning {
        QuestWarning::SummaryMismatch => QuestWarningOutput::SummaryMismatch,
        QuestWarning::AllNotStarted => QuestWarningOutput::AllNotStarted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runelite::parse_quest_export;

    const SYNTHETIC: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/quests.synthetic.json");

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn browse(offset: usize, limit: usize) -> Query {
        Query {
            state: None,
            page: Page { offset, limit },
        }
    }

    /// An export with one quest per state letter: `F`, `I` or `N`, ids 1, 2, ...
    fn export_of(states: &str, summary: Option<&str>) -> QuestProgress {
        let rows: Vec<String> = states
            .chars()
            .enumerate()
            .map(|(index, letter)| {
                let state = match letter {
                    'F' => "FINISHED",
                    'I' => "IN_PROGRESS",
                    _ => "NOT_STARTED",
                };
                format!(
                    r#"{{"id":{},"name":"Quest {index}","state":"{state}"}}"#,
                    index + 1
                )
            })
            .collect();
        let summary = summary.map_or_else(String::new, |text| format!(r#""summary":{text},"#));
        let json = format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z",{summary}"quests":[{}]}}"#,
            rows.join(",")
        );
        parse_quest_export(json.as_bytes()).unwrap()
    }

    fn ids(result: &QuestStatus) -> Vec<u32> {
        result
            .quests
            .iter()
            .map(|quest| quest.runelite_id)
            .collect()
    }

    #[test]
    fn a_synthetic_export_is_reported_with_both_timestamps_and_runelite_ids() {
        let progress = parse_quest_export(SYNTHETIC).unwrap();

        let result = quest_status(&progress, &browse(0, 100), ts("2026-02-01T13:12:13Z"));

        assert_eq!(result.status, ContentsStatus::Ok);
        assert_eq!(result.observed.read_at, "2026-02-01T13:12:13Z");
        assert_eq!(ids(&result), [203, 18, 90]);
        assert_eq!(
            result
                .quests
                .iter()
                .map(|quest| quest.state)
                .collect::<Vec<_>>(),
            [
                QuestStateOutput::Finished,
                QuestStateOutput::InProgress,
                QuestStateOutput::NotStarted
            ]
        );
        assert_eq!(result.matched, 3);
        assert!(!result.truncated);
        assert_eq!(result.warnings, []);
    }

    #[test]
    fn counts_cover_every_recorded_quest_whatever_the_filter_and_page() {
        let progress = export_of("FFINNNF", None);
        let query = Query {
            state: Some(QuestStateOutput::Finished),
            page: Page {
                offset: 1,
                limit: 1,
            },
        };

        let result = quest_status(&progress, &query, ts("2026-01-01T12:00:00Z"));

        assert_eq!(
            result.counts,
            QuestCounts {
                recorded: 7,
                finished: 3,
                in_progress: 1,
                not_started: 3
            }
        );
    }

    #[test]
    fn the_state_filter_applies_before_paging() {
        // Finished quests are ids 1, 2 and 7; in_progress is 3; the rest are not started.
        let progress = export_of("FFINNNF", None);
        let now = ts("2026-01-01T12:00:00Z");
        let finished = |offset, limit| Query {
            state: Some(QuestStateOutput::Finished),
            page: Page { offset, limit },
        };

        let first = quest_status(&progress, &finished(0, 2), now);
        let second = quest_status(&progress, &finished(2, 2), now);

        assert_eq!(ids(&first), [1, 2]);
        assert_eq!((first.matched, first.truncated), (3, true));
        assert_eq!(ids(&second), [7]);
        assert_eq!((second.matched, second.truncated), (3, false));
        assert!(
            first
                .quests
                .iter()
                .all(|quest| quest.state == QuestStateOutput::Finished)
        );
    }

    #[test]
    fn a_filter_matching_nothing_is_an_empty_ok_page() {
        let progress = export_of("FF", None);
        let query = Query {
            state: Some(QuestStateOutput::InProgress),
            page: Page {
                offset: 0,
                limit: 10,
            },
        };

        let result = quest_status(&progress, &query, ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.quests, []);
        assert_eq!((result.matched, result.counts.recorded), (0, 2));
        assert!(!result.truncated);
    }

    #[test]
    fn an_offset_past_the_end_is_an_empty_page_not_a_failure() {
        let result = quest_status(
            &export_of("FFF", None),
            &browse(10, 5),
            ts("2026-01-01T12:00:00Z"),
        );

        assert_eq!(result.quests, []);
        assert_eq!(result.matched, 3);
        assert!(!result.truncated);
    }

    #[test]
    fn a_page_exactly_covering_the_rest_is_not_truncated() {
        let result = quest_status(
            &export_of("FFFF", None),
            &browse(2, 2),
            ts("2026-01-01T12:00:00Z"),
        );

        assert_eq!(ids(&result), [3, 4]);
        assert!(!result.truncated);
    }

    #[test]
    fn an_exported_empty_list_is_ok_with_zero_recorded() {
        let result = quest_status(
            &export_of("", None),
            &browse(0, 100),
            ts("2026-01-01T12:00:00Z"),
        );

        assert_eq!(result.counts.recorded, 0);
        assert_eq!(result.quests, []);
    }

    #[test]
    fn warnings_are_reported_without_changing_the_quests() {
        let mismatch = export_of(
            "FI",
            Some(r#"{"finished":5,"in_progress":0,"not_started":0}"#),
        );
        let fresh = export_of("NNN", None);
        let now = ts("2026-01-01T12:00:00Z");

        let mismatch = quest_status(&mismatch, &browse(0, 10), now);
        let fresh = quest_status(&fresh, &browse(0, 10), now);

        assert_eq!(mismatch.warnings, [QuestWarningOutput::SummaryMismatch]);
        assert_eq!(mismatch.quests.len(), 2);
        assert_eq!(fresh.warnings, [QuestWarningOutput::AllNotStarted]);
        assert_eq!(fresh.quests.len(), 3);
    }

    #[test]
    fn long_names_are_cut_and_flagged_without_changing_the_id() {
        let long = "x".repeat(MAX_QUEST_NAME_CHARS + 5);
        let json = format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","quests":[
                {{"id":7,"name":"Short","state":"FINISHED"}},
                {{"id":8,"name":"{long}","state":"FINISHED"}}]}}"#
        );
        let progress = parse_quest_export(json.as_bytes()).unwrap();

        let result = quest_status(&progress, &browse(0, 10), ts("2026-01-01T12:00:00Z"));

        assert!(!result.quests[0].name_truncated);
        assert!(result.quests[1].name_truncated);
        assert_eq!(result.quests[1].name.chars().count(), MAX_QUEST_NAME_CHARS);
        assert_eq!(result.quests[1].runelite_id, 8);
    }

    #[test]
    fn a_future_export_time_gives_no_age() {
        let result = quest_status(
            &export_of("F", None),
            &browse(0, 10),
            ts("2025-12-31T00:00:00Z"),
        );

        assert_eq!(result.observed.seconds_since_last_change, None);
        assert!(result.observed.exported_at_in_future);
    }

    #[test]
    fn omitted_arguments_browse_the_first_default_page() {
        let query = QuestStatusInput {
            state: None,
            offset: None,
            limit: None,
        }
        .into_query();

        assert_eq!(query, Ok(browse(0, DEFAULT_LIMIT)));
    }

    #[test]
    fn limits_outside_one_to_the_maximum_are_rejected() {
        let over = u32::try_from(MAX_LIMIT).unwrap() + 1;
        for limit in [0, over, u32::MAX] {
            let input = QuestStatusInput {
                state: None,
                offset: None,
                limit: Some(limit),
            };
            assert!(input.into_query().is_err(), "limit {limit}");
        }
        let at_max = QuestStatusInput {
            state: Some(QuestStateOutput::NotStarted),
            offset: Some(3),
            limit: Some(over - 1),
        };
        assert_eq!(
            at_max.into_query(),
            Ok(Query {
                state: Some(QuestStateOutput::NotStarted),
                page: Page {
                    offset: 3,
                    limit: MAX_LIMIT
                }
            })
        );
    }

    #[test]
    fn failures_name_quests_json_and_never_a_path() {
        let failure = quest_failure(&ExportReadError::FileNotFound);

        assert!(
            failure.message.contains("quests.json"),
            "{}",
            failure.message
        );
    }
}
