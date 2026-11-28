//! The `diary_status` tool: achievement diary observations from the local
//! Character Exporter file.
//!
//! This module only adapts. Reading is [`LocalExportSource`](crate::runelite::LocalExportSource)
//! and parsing is [`parse_diary_export`](crate::runelite::parse_diary_export).
//! The exporter reports three independent observations per tier: a completion
//! flag, a count of done tasks and an optional list of named tasks with their
//! own flags. They often disagree, so they are reported side by side and never
//! reconciled, and nothing here claims that a diary is complete. Area names are
//! untrusted text; nothing maps tasks to Wiki pages or requirements.

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
    ExportFailure, ObservationTime, bounded, bounded_name_to, failure_for, into_result,
};
use crate::runelite::{
    DIARY_FILE_NAME, DiaryArea, DiaryProgress, DiaryTask, DiaryTier, DiaryTierProgress,
    DiaryWarning, ExportReadError,
};

/// Areas returned when `limit` is not given.
const DEFAULT_LIMIT: usize = 25;
/// Most areas one call can return, the parser's own area bound.
const MAX_LIMIT: usize = 64;
/// Longest area or task name returned, in characters. Names come from the file.
const MAX_NAME_CHARS: usize = 64;
/// Longest `area` filter accepted, in bytes.
const MAX_AREA_FILTER_BYTES: usize = 128;
/// Most warnings returned.
const MAX_WARNINGS: usize = 64;

/// A diary tier as recorded by the exporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum DiaryTierOutput {
    /// Easy diary.
    Easy,
    /// Medium diary.
    Medium,
    /// Hard diary.
    Hard,
    /// Elite diary.
    Elite,
}

impl From<DiaryTier> for DiaryTierOutput {
    fn from(tier: DiaryTier) -> Self {
        match tier {
            DiaryTier::Easy => Self::Easy,
            DiaryTier::Medium => Self::Medium,
            DiaryTier::Hard => Self::Hard,
            DiaryTier::Elite => Self::Elite,
        }
    }
}

/// Arguments of the `diary_status` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct DiaryStatusInput {
    /// Only this area, matched exactly but ignoring case. The counts in the
    /// response ignore this filter.
    area: Option<String>,
    /// Only this tier within each area.
    tier: Option<DiaryTierOutput>,
    /// Also return each tier's named tasks. Allowed only together with `area`,
    /// so a response cannot carry every task of every diary. Defaults to false.
    include_tasks: Option<bool>,
    /// Matching areas to skip, in name order. Defaults to 0.
    offset: Option<u32>,
    /// Most areas to return, 1 to 64. Defaults to 25.
    limit: Option<u32>,
}

/// A validated request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Query {
    /// The requested area name, lower-cased for comparison.
    area: Option<String>,
    tier: Option<DiaryTierOutput>,
    include_tasks: bool,
    offset: usize,
    limit: usize,
}

impl DiaryStatusInput {
    fn into_query(self) -> Result<Query, String> {
        let area = self.area.map(|name| name.trim().to_lowercase());
        if area
            .as_deref()
            .is_some_and(|name| name.is_empty() || name.len() > MAX_AREA_FILTER_BYTES)
        {
            return Err(format!(
                "area must be 1 to {MAX_AREA_FILTER_BYTES} bytes after trimming"
            ));
        }
        let include_tasks = self.include_tasks.unwrap_or(false);
        if include_tasks && area.is_none() {
            return Err("include_tasks requires area".to_owned());
        }
        let limit = match self.limit {
            None => DEFAULT_LIMIT,
            Some(limit) => usize::try_from(limit)
                .ok()
                .filter(|limit| (1..=MAX_LIMIT).contains(limit))
                .ok_or_else(|| format!("limit must be between 1 and {MAX_LIMIT}"))?,
        };
        let offset = self
            .offset
            .map_or(Ok(0), usize::try_from)
            .map_err(|_| "offset is too large".to_owned())?;
        Ok(Query {
            area,
            tier: self.tier,
            include_tasks,
            offset,
            limit,
        })
    }
}

/// Totals over the whole export, whatever the filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DiaryCounts {
    /// Areas present in the export. An area absent from it is unrecorded.
    pub areas_recorded: usize,
    /// Tiers present across all areas. A missing tier is unrecorded.
    pub tiers_recorded: usize,
    /// Tiers whose exporter completion flag is set. This counts a flag the
    /// exporter wrote; it is not a verdict that those diaries are complete.
    pub tiers_flagged_complete: usize,
}

/// One named task as listed by the exporter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DiaryTaskOutput {
    /// Task text from the file, untrusted, cut to 64 characters.
    pub name: String,
    /// True if `name` was cut short and is therefore not the full name.
    pub name_truncated: bool,
    /// The exporter's flag for this task.
    pub complete: bool,
}

/// The exporter's observations for one tier, reported independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DiaryTierOutputRow {
    /// The tier.
    pub tier: DiaryTierOutput,
    /// The exporter's tier completion flag. An observation that can disagree
    /// with `tasks_done` and with the listed tasks; not authoritative.
    pub flagged_complete: bool,
    /// The exporter's count of done tasks. An observation, not authoritative.
    pub tasks_done: u16,
    /// Number of named tasks the file lists for this tier. Null when the file
    /// has no task list for the tier (unrecorded, not zero).
    pub tasks_listed: Option<usize>,
    /// How many of the listed tasks carry a complete flag. Null when no list.
    pub tasks_listed_complete: Option<usize>,
    /// The listed tasks, only with `include_tasks`; null otherwise.
    pub tasks: Option<Vec<DiaryTaskOutput>>,
}

/// One area and the tiers recorded for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DiaryAreaOutput {
    /// Area name from the file, untrusted, cut to 64 characters.
    pub name: String,
    /// True if `name` was cut short and is therefore not the full name.
    pub name_truncated: bool,
    /// Recorded tiers in difficulty order. A tier not listed is unrecorded.
    pub tiers: Vec<DiaryTierOutputRow>,
}

/// A non-fatal finding. It never changes the observations listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiaryWarningOutput {
    /// The tier's `tasks_done` count differs from the listed tasks flagged complete.
    TaskCountMismatch {
        /// The area name, cut to 64 characters.
        area: String,
        /// The tier.
        tier: DiaryTierOutput,
    },
    /// The tier is flagged complete but a listed task is not.
    CompleteWithIncompleteTask {
        /// The area name, cut to 64 characters.
        area: String,
        /// The tier.
        tier: DiaryTierOutput,
    },
    /// The area has a tier name this server does not recognise; it is omitted.
    UnknownTier {
        /// The area name, cut to 64 characters.
        area: String,
    },
    /// The file's own summary contradicts its observations.
    SummaryMismatch,
}

/// Successful result of `diary_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DiaryStatus {
    /// Always `ok`.
    pub status: ContentsStatus,
    /// Timing of this file only. Other files have their own timestamps and
    /// are not guaranteed to be consistent with this one.
    #[serde(flatten)]
    pub observed: ObservationTime,
    /// Totals over every recorded area, ignoring all filters and paging.
    pub counts: DiaryCounts,
    /// The requested page of matching areas, in name order.
    pub areas: Vec<DiaryAreaOutput>,
    /// Areas matching the `area` and `tier` filters, counted before paging.
    pub matched: usize,
    /// Matching areas skipped before this page.
    pub offset: usize,
    /// The page size applied.
    pub limit: usize,
    /// True if matching areas after this page exist.
    pub truncated: bool,
    /// Non-fatal findings, at most 64.
    pub warnings: Vec<DiaryWarningOutput>,
    /// How many warnings the import produced.
    pub warning_count: usize,
    /// True if `warnings` omits some warnings.
    pub warnings_truncated: bool,
}

#[tool_router(router = diaries_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Reports achievement diary observations from the local Character Exporter file.
    #[tool(
        name = "diary_status",
        description = "Read the exporter's achievement diary observations from the local \
                       RuneLite Character Exporter file configured for this server, with the \
                       exporter timestamp and import warnings. Reads the file on every call. \
                       Per area and tier it reports three independent observations: \
                       flagged_complete (the exporter's tier flag), tasks_done (its count) and, \
                       when the file lists tasks, tasks_listed and tasks_listed_complete. They \
                       often disagree and are never reconciled; none is authoritative \
                       completion, and warnings name the disagreements. Optionally filter by \
                       area (exact name, ignoring case) and tier; offset and limit (default 25, \
                       at most 64) page the matching areas in name order. Task names are \
                       returned only with include_tasks, which requires area. counts covers \
                       every recorded area and ignores filters. A missing area or tier is \
                       unrecorded, not incomplete. The timestamp is the last change to the \
                       diary file, not the time of reading, and the file is not consistent \
                       with any other export file. A missing file is reported as \
                       file_not_found. Does not check requirements or say which diaries are \
                       available. Failures report a status such as not_configured or \
                       file_not_found.",
        annotations(
            title = "Diary status",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        output_schema = schema_for_output::<DiaryStatus>()
    )]
    async fn diary_status(
        &self,
        Parameters(input): Parameters<DiaryStatusInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = input
            .into_query()
            .map_err(|message| ErrorData::invalid_params(message, None))?;

        // The read is blocking file I/O, so it stays off the async executor.
        let source = self.export_source.clone();
        let read = tokio::task::spawn_blocking(move || source.read_diaries())
            .await
            .map_err(|_| ErrorData::internal_error("the export read task failed", None))?;

        // One clock sample, taken after the read, serves both `read_at` and the age.
        let outcome = read
            .map(|progress| diary_status(&progress, &query, Timestamp::now()))
            .map_err(|error| diary_failure(&error));
        into_result(outcome)
    }
}

/// The fixed failure for `diaries.json`.
fn diary_failure(error: &ExportReadError) -> ExportFailure {
    failure_for(error, DIARY_FILE_NAME)
}

/// Builds the response for a parsed export, read at `read_at`.
fn diary_status(progress: &DiaryProgress, query: &Query, read_at: Timestamp) -> DiaryStatus {
    // Filtering looks at every area; the page is taken afterwards.
    let matching: Vec<(&String, &DiaryArea)> = progress
        .areas()
        .iter()
        .filter(|(name, area)| matches_filters(name, area, query))
        .collect();
    let shown: Vec<DiaryAreaOutput> = matching
        .iter()
        .skip(query.offset)
        .take(query.limit)
        .map(|(name, area)| area_output(name, area, query))
        .collect();
    let area_names: Vec<&String> = progress.areas().keys().collect();
    let (warnings, warning_count) = bounded(progress.warnings(), MAX_WARNINGS, |warning| {
        warning_output(*warning, &area_names)
    });

    DiaryStatus {
        status: ContentsStatus::Ok,
        observed: ObservationTime::new(progress.exported_at(), progress.plugin_version(), read_at),
        counts: counts(progress),
        truncated: query.offset.saturating_add(shown.len()) < matching.len(),
        areas: shown,
        matched: matching.len(),
        offset: query.offset,
        limit: query.limit,
        warnings_truncated: warning_count > warnings.len(),
        warnings,
        warning_count,
    }
}

/// Whether `area` passes the `area` and `tier` filters. An area with no
/// tier of the requested kind does not match.
fn matches_filters(name: &str, area: &DiaryArea, query: &Query) -> bool {
    let named = query
        .area
        .as_deref()
        .is_none_or(|wanted| name.to_lowercase() == wanted);
    let has_tier = query.tier.is_none_or(|wanted| {
        area.tiers()
            .keys()
            .any(|tier| DiaryTierOutput::from(*tier) == wanted)
    });
    named && has_tier
}

fn counts(progress: &DiaryProgress) -> DiaryCounts {
    let tiers = || {
        progress
            .areas()
            .values()
            .flat_map(|area| area.tiers().values())
    };
    DiaryCounts {
        areas_recorded: progress.areas().len(),
        tiers_recorded: tiers().count(),
        tiers_flagged_complete: tiers().filter(|tier| tier.complete).count(),
    }
}

fn area_output(name: &str, area: &DiaryArea, query: &Query) -> DiaryAreaOutput {
    let (name, name_truncated) = bounded_name_to(name, MAX_NAME_CHARS);
    let tiers = area
        .tiers()
        .iter()
        .filter(|(tier, _)| {
            query
                .tier
                .is_none_or(|wanted| DiaryTierOutput::from(**tier) == wanted)
        })
        .map(|(tier, progress)| tier_output(*tier, progress, query.include_tasks))
        .collect();
    DiaryAreaOutput {
        name,
        name_truncated,
        tiers,
    }
}

fn tier_output(
    tier: DiaryTier,
    progress: &DiaryTierProgress,
    include_tasks: bool,
) -> DiaryTierOutputRow {
    let listed = progress.tasks.as_deref();
    DiaryTierOutputRow {
        tier: tier.into(),
        flagged_complete: progress.complete,
        tasks_done: progress.tasks_done,
        tasks_listed: listed.map(<[DiaryTask]>::len),
        tasks_listed_complete: listed
            .map(|tasks| tasks.iter().filter(|task| task.complete).count()),
        tasks: listed
            .filter(|_| include_tasks)
            .map(|tasks| tasks.iter().map(task_output).collect()),
    }
}

fn task_output(task: &DiaryTask) -> DiaryTaskOutput {
    let (name, name_truncated) = bounded_name_to(&task.display_name, MAX_NAME_CHARS);
    DiaryTaskOutput {
        name,
        name_truncated,
        complete: task.complete,
    }
}

/// Maps a parser warning to its output, naming the area the parser located by
/// its one-based position in name order.
fn warning_output(warning: DiaryWarning, area_names: &[&String]) -> DiaryWarningOutput {
    let area = |position: usize| {
        area_names
            .get(position.saturating_sub(1))
            .map_or_else(String::new, |name| bounded_name_to(name, MAX_NAME_CHARS).0)
    };
    match warning {
        DiaryWarning::TaskCountMismatch {
            area: position,
            tier,
        } => DiaryWarningOutput::TaskCountMismatch {
            area: area(position),
            tier: tier.into(),
        },
        DiaryWarning::CompleteWithIncompleteTask {
            area: position,
            tier,
        } => DiaryWarningOutput::CompleteWithIncompleteTask {
            area: area(position),
            tier: tier.into(),
        },
        DiaryWarning::UnknownTier { area: position } => DiaryWarningOutput::UnknownTier {
            area: area(position),
        },
        DiaryWarning::SummaryMismatch => DiaryWarningOutput::SummaryMismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runelite::parse_diary_export;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    /// Alpha has a consistent easy tier and a medium tier whose flag, count and
    /// list disagree; Beta has only an easy tier with no task list; Gamma has
    /// an unknown tier next to a hard tier.
    fn sample() -> DiaryProgress {
        let json = r#"{
          "exported_at": "2026-01-01T12:00:00Z",
          "diaries": {
            "Alpha": {
              "easy": {"complete": true, "tasks_done": 2, "tasks": [
                {"name": "A1", "complete": true}, {"name": "A2", "complete": true}]},
              "medium": {"complete": true, "tasks_done": 5, "tasks": [
                {"name": "M1", "complete": true}, {"name": "M2", "complete": false}]}
            },
            "Beta": { "easy": {"complete": false, "tasks_done": 0} },
            "Gamma": { "hard": {"complete": false, "tasks_done": 1}, "mystery": {"complete": true, "tasks_done": 0} }
          }
        }"#;
        parse_diary_export(json.as_bytes()).unwrap()
    }

    fn query() -> Query {
        Query {
            area: None,
            tier: None,
            include_tasks: false,
            offset: 0,
            limit: DEFAULT_LIMIT,
        }
    }

    fn input() -> DiaryStatusInput {
        DiaryStatusInput {
            area: None,
            tier: None,
            include_tasks: None,
            offset: None,
            limit: None,
        }
    }

    fn run(query: &Query) -> DiaryStatus {
        diary_status(&sample(), query, ts("2026-01-01T12:01:00Z"))
    }

    fn names(result: &DiaryStatus) -> Vec<&str> {
        result.areas.iter().map(|area| area.name.as_str()).collect()
    }

    #[test]
    fn the_three_observations_are_reported_side_by_side_without_reconciling() {
        let result = run(&query());

        let medium = &result.areas[0].tiers[1];
        assert_eq!(medium.tier, DiaryTierOutput::Medium);
        assert!(medium.flagged_complete);
        assert_eq!(medium.tasks_done, 5);
        assert_eq!(
            (medium.tasks_listed, medium.tasks_listed_complete),
            (Some(2), Some(1))
        );
        assert_eq!(medium.tasks, None);
    }

    #[test]
    fn a_tier_without_a_task_list_has_null_list_counts_not_zero() {
        let result = run(&query());

        let beta = &result.areas[1].tiers[0];
        assert_eq!(
            (beta.tasks_listed, beta.tasks_listed_complete),
            (None, None)
        );
    }

    #[test]
    fn counts_cover_everything_whatever_the_filters_and_page() {
        let filtered = Query {
            area: Some("beta".to_owned()),
            ..query()
        };

        let result = run(&filtered);

        assert_eq!(
            result.counts,
            DiaryCounts {
                areas_recorded: 3,
                tiers_recorded: 4,
                tiers_flagged_complete: 2
            }
        );
        assert_eq!(names(&result), ["Beta"]);
        assert_eq!(result.matched, 1);
    }

    #[test]
    fn tasks_are_returned_only_with_include_tasks_for_a_named_area() {
        let wanted = Query {
            area: Some("alpha".to_owned()),
            include_tasks: true,
            ..query()
        };

        let result = run(&wanted);

        let easy = &result.areas[0].tiers[0];
        let listed: Vec<(&str, bool)> = easy
            .tasks
            .as_ref()
            .unwrap()
            .iter()
            .map(|task| (task.name.as_str(), task.complete))
            .collect();
        assert_eq!(listed, [("A1", true), ("A2", true)]);
    }

    #[test]
    fn the_tier_filter_applies_inside_areas_and_drops_areas_without_it() {
        let hard = Query {
            tier: Some(DiaryTierOutput::Hard),
            ..query()
        };

        let result = run(&hard);

        assert_eq!(names(&result), ["Gamma"]);
        assert_eq!(result.areas[0].tiers.len(), 1);
        assert_eq!(result.areas[0].tiers[0].tier, DiaryTierOutput::Hard);
        assert_eq!(result.matched, 1);
    }

    #[test]
    fn areas_are_filtered_before_paging() {
        let easy = |offset, limit| Query {
            tier: Some(DiaryTierOutput::Easy),
            offset,
            limit,
            ..query()
        };

        let first = run(&easy(0, 1));
        let second = run(&easy(1, 1));

        assert_eq!(
            (names(&first), first.matched, first.truncated),
            (vec!["Alpha"], 2, true)
        );
        assert_eq!(
            (names(&second), second.matched, second.truncated),
            (vec!["Beta"], 2, false)
        );
    }

    #[test]
    fn an_unrecorded_area_is_an_empty_ok_result() {
        let missing = Query {
            area: Some("nowhere".to_owned()),
            ..query()
        };

        let result = run(&missing);

        assert_eq!(result.areas, []);
        assert_eq!((result.matched, result.counts.areas_recorded), (0, 3));
    }

    #[test]
    fn warnings_name_the_area_and_never_change_the_observations() {
        let result = run(&query());

        assert!(
            result
                .warnings
                .contains(&DiaryWarningOutput::TaskCountMismatch {
                    area: "Alpha".to_owned(),
                    tier: DiaryTierOutput::Medium
                })
        );
        assert!(
            result
                .warnings
                .contains(&DiaryWarningOutput::CompleteWithIncompleteTask {
                    area: "Alpha".to_owned(),
                    tier: DiaryTierOutput::Medium
                })
        );
        assert!(result.warnings.contains(&DiaryWarningOutput::UnknownTier {
            area: "Gamma".to_owned()
        }));
        assert_eq!(result.warning_count, result.warnings.len());
        assert!(!result.warnings_truncated);
        // The contradictory tier is still reported exactly as exported.
        assert!(result.areas[0].tiers[1].flagged_complete);
    }

    #[test]
    fn a_future_export_time_gives_no_age() {
        let result = diary_status(&sample(), &query(), ts("2025-12-31T00:00:00Z"));

        assert_eq!(result.observed.seconds_since_last_change, None);
        assert!(result.observed.exported_at_in_future);
    }

    #[test]
    fn long_names_are_cut_and_flagged() {
        let long = "x".repeat(MAX_NAME_CHARS + 5);
        let json = format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","diaries":{{"{long}":{{"easy":{{"complete":false,"tasks_done":0,"tasks":[{{"name":"{long}","complete":false}}]}}}}}}}}"#
        );
        let progress = parse_diary_export(json.as_bytes()).unwrap();
        let wanted = Query {
            area: Some(long),
            include_tasks: true,
            ..query()
        };

        let result = diary_status(&progress, &wanted, ts("2026-01-01T12:00:00Z"));

        assert!(result.areas[0].name_truncated);
        assert_eq!(result.areas[0].name.chars().count(), MAX_NAME_CHARS);
        let task = &result.areas[0].tiers[0].tasks.as_ref().unwrap()[0];
        assert!(task.name_truncated);
    }

    #[test]
    fn omitted_arguments_browse_the_first_default_page() {
        assert_eq!(input().into_query(), Ok(query()));
    }

    #[test]
    fn include_tasks_without_an_area_is_rejected() {
        let without_area = DiaryStatusInput {
            include_tasks: Some(true),
            ..input()
        };
        let with_area = DiaryStatusInput {
            include_tasks: Some(true),
            area: Some("Alpha".to_owned()),
            ..input()
        };

        assert!(without_area.into_query().is_err());
        assert!(with_area.into_query().is_ok());
    }

    #[test]
    fn empty_oversized_areas_and_bad_limits_are_rejected() {
        let area = |text: &str| DiaryStatusInput {
            area: Some(text.to_owned()),
            ..input()
        };
        assert!(area("  ").into_query().is_err());
        assert!(
            area(&"x".repeat(MAX_AREA_FILTER_BYTES + 1))
                .into_query()
                .is_err()
        );
        for limit in [0, u32::try_from(MAX_LIMIT).unwrap() + 1] {
            let bad = DiaryStatusInput {
                limit: Some(limit),
                ..input()
            };
            assert!(bad.into_query().is_err(), "limit {limit}");
        }
    }

    #[test]
    fn failures_name_diaries_json_and_never_a_path() {
        let failure = diary_failure(&ExportReadError::FileNotFound);

        assert!(
            failure.message.contains("diaries.json"),
            "{}",
            failure.message
        );
    }
}
