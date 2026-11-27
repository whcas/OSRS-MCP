//! The `bank_contents` tool: bank holdings from the local Character Exporter
//! file.
//!
//! This module only adapts. Reading is [`LocalExportSource`](crate::runelite::LocalExportSource)
//! and parsing is [`parse_container_export`](crate::runelite::parse_container_export);
//! here a parsed snapshot becomes a bounded page of rows and a read failure
//! becomes a fixed, safe status. Item IDs identify items; names are untrusted
//! display text. No prices, valuation or totals are computed, and no account
//! name, world, session id, path or raw JSON is returned.

use jiff::Timestamp;
use rmcp::{
    handler::server::{tool::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, ErrorData},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};

use super::OsrsMcpServer;
use super::export_output::{
    ExportFailure, ObservationTime, bounded, bounded_name, failure_for, into_result,
};
use crate::runelite::{
    ContainerKind, ContainerRow, ContainerSnapshot, ContainerWarning, ExportReadError,
};

/// Rows returned when `limit` is not given.
const DEFAULT_LIMIT: usize = 100;
/// Most rows one call can return.
const MAX_LIMIT: usize = 200;
/// Most warnings returned.
const MAX_WARNINGS: usize = 64;

/// The page of rows to return.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct PageInput {
    /// Rows to skip, in the order the file lists them. Defaults to 0.
    offset: Option<u32>,
    /// Most rows to return, 1 to 200. Defaults to 100.
    limit: Option<u32>,
}

/// A validated page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Page {
    offset: usize,
    limit: usize,
}

impl PageInput {
    fn into_page(self) -> Result<Page, String> {
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
        Ok(Page { offset, limit })
    }
}

/// Success marker, so a client can tell results apart by `status` alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ContentsStatus {
    /// The file was read and imported. Warnings may be present.
    Ok,
}

/// Which container the rows describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ContainerOutput {
    /// Bank holdings as last observed by the exporter.
    Bank,
    /// The player's inventory.
    Inventory,
    /// The player's equipped items.
    Equipment,
}

/// One non-empty slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ContainerRowOutput {
    /// Zero-based index in the exported container.
    pub slot: u16,
    /// Canonical item identifier. Different slots can hold the same item.
    pub item_id: u32,
    /// Number of items in this slot, as exported.
    pub quantity: u64,
    /// Display name from the file, if it has one. Untrusted text: identify the
    /// item by `item_id`.
    pub name: Option<String>,
    /// True if `name` was cut short and is therefore not the full name.
    pub name_truncated: bool,
}

/// A non-fatal finding from the import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContainerWarningOutput {
    /// The file's own `item_count` differs from the number of rows it lists.
    /// Rows are reported as listed.
    ItemCountMismatch,
    /// A row uses an equipment slot outside the documented range.
    UnknownEquipmentSlot {
        /// The row's 1-based position in the file.
        row: usize,
    },
}

/// Successful result of a container tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ContainerContents {
    /// Always `ok`.
    pub status: ContentsStatus,
    /// Which container the rows describe.
    pub container: ContainerOutput,
    /// Timing of this file only. Other files have their own timestamps and
    /// are not guaranteed to be consistent with this one.
    #[serde(flatten)]
    pub observed: ObservationTime,
    /// The requested page, in the order the file lists the rows. Repeated item
    /// IDs are separate rows and are not merged.
    pub rows: Vec<ContainerRowOutput>,
    /// Rows in the whole file. An empty container reports 0 with status `ok`.
    pub total_rows: usize,
    /// Rows skipped before this page.
    pub offset: usize,
    /// The page size applied.
    pub limit: usize,
    /// True if rows after this page exist (`offset + rows.len() < total_rows`).
    pub truncated: bool,
    /// Non-fatal findings, at most 64.
    pub warnings: Vec<ContainerWarningOutput>,
    /// How many warnings the import produced.
    pub warning_count: usize,
    /// True if `warnings` omits some warnings.
    pub warnings_truncated: bool,
}

#[tool_router(router = containers_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Reports bank holdings from the local Character Exporter file.
    #[tool(
        name = "bank_contents",
        description = "Read the player's bank from the local RuneLite Character Exporter file \
                       configured for this server: item ids, quantities and optional display \
                       names per slot, with the exporter timestamp and import warnings. Reads \
                       the file on every call. Paginated with offset and limit (default 100, \
                       at most 200 rows). The timestamp is the last change to the bank file, \
                       not the time of reading, and the exporter only writes the bank after it \
                       has been opened, so the file can be sessions old. A missing file is \
                       reported as file_not_found, never as an empty bank. Does not price \
                       items or total their value, and is not consistent with any other \
                       export file. Failures report a status such as not_configured or \
                       file_not_found.",
        annotations(
            title = "Bank contents",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        output_schema = schema_for_output::<ContainerContents>()
    )]
    async fn bank_contents(
        &self,
        Parameters(input): Parameters<PageInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = input
            .into_page()
            .map_err(|message| ErrorData::invalid_params(message, None))?;
        self.container_result(ContainerKind::Bank, page).await
    }
}

impl OsrsMcpServer {
    /// Reads one container off the async executor and builds the tool result.
    async fn container_result(
        &self,
        kind: ContainerKind,
        page: Page,
    ) -> Result<CallToolResult, ErrorData> {
        let source = self.export_source.clone();
        let read = tokio::task::spawn_blocking(move || source.read_container(kind))
            .await
            .map_err(|_| ErrorData::internal_error("the export read task failed", None))?;

        // One clock sample, taken after the read, serves both `read_at` and the age.
        let outcome = read
            .map(|snapshot| contents(&snapshot, page, Timestamp::now()))
            .map_err(|error| container_failure(&error, kind));
        into_result(outcome)
    }
}

/// The fixed failure for `kind`, naming its file.
fn container_failure(error: &ExportReadError, kind: ContainerKind) -> ExportFailure {
    failure_for(error, crate::runelite::container_file_name(kind))
}

/// Builds the response for a parsed snapshot, read at `read_at`.
fn contents(snapshot: &ContainerSnapshot, page: Page, read_at: Timestamp) -> ContainerContents {
    let rows = snapshot.rows();
    let (warnings, warning_count) = bounded(snapshot.warnings(), MAX_WARNINGS, warning_output);
    let shown: Vec<ContainerRowOutput> = rows
        .iter()
        .skip(page.offset)
        .take(page.limit)
        .map(row_output)
        .collect();

    ContainerContents {
        status: ContentsStatus::Ok,
        container: container_output(snapshot.kind()),
        observed: ObservationTime::new(snapshot.exported_at(), snapshot.plugin_version(), read_at),
        truncated: page.offset.saturating_add(shown.len()) < rows.len(),
        rows: shown,
        total_rows: rows.len(),
        offset: page.offset,
        limit: page.limit,
        warnings_truncated: warning_count > warnings.len(),
        warnings,
        warning_count,
    }
}

const fn container_output(kind: ContainerKind) -> ContainerOutput {
    match kind {
        ContainerKind::Bank => ContainerOutput::Bank,
        ContainerKind::Inventory => ContainerOutput::Inventory,
        ContainerKind::Equipment => ContainerOutput::Equipment,
    }
}

fn row_output(row: &ContainerRow) -> ContainerRowOutput {
    let named = row.display_name.as_deref().map(bounded_name);
    let name_truncated = named.as_ref().is_some_and(|(_, truncated)| *truncated);
    ContainerRowOutput {
        slot: row.slot,
        item_id: row.item_id.0,
        quantity: row.quantity.get(),
        name: named.map(|(name, _)| name),
        name_truncated,
    }
}

const fn warning_output(warning: &ContainerWarning) -> ContainerWarningOutput {
    match warning {
        ContainerWarning::ItemCountMismatch => ContainerWarningOutput::ItemCountMismatch,
        ContainerWarning::UnknownEquipmentSlot { row } => {
            ContainerWarningOutput::UnknownEquipmentSlot { row: *row }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runelite::parse_container_export;

    const BANK: &[u8] = include_bytes!("../../tests/fixtures/runelite_export/bank.synthetic.json");

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    /// A bank export whose `count` rows hold item 1 in slots 0, 1, 2, ...
    fn bank_with_rows(count: usize) -> ContainerSnapshot {
        let rows: Vec<String> = (0..count)
            .map(|slot| {
                format!(
                    r#"{{"slot":{slot},"id":1,"quantity":{},"name":"Row"}}"#,
                    slot + 1
                )
            })
            .collect();
        let json = format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","kind":"bank","item_count":{count},"items":[{}]}}"#,
            rows.join(",")
        );
        parse_container_export(ContainerKind::Bank, json.as_bytes()).unwrap()
    }

    fn page(offset: usize, limit: usize) -> Page {
        Page { offset, limit }
    }

    #[test]
    fn a_synthetic_bank_is_reported_with_both_timestamps_and_in_file_order() {
        let snapshot = parse_container_export(ContainerKind::Bank, BANK).unwrap();

        let result = contents(&snapshot, page(0, 100), ts("2026-01-01T12:01:00Z"));

        assert_eq!(result.status, ContentsStatus::Ok);
        assert_eq!(result.container, ContainerOutput::Bank);
        assert_eq!(result.total_rows, snapshot.rows().len());
        assert_eq!(result.rows.len(), snapshot.rows().len());
        assert!(!result.truncated);
        assert_eq!(result.observed.read_at, "2026-01-01T12:01:00Z");
        assert_eq!(result.observed.seconds_since_last_change, Some(60));
        let slots: Vec<u16> = result.rows.iter().map(|row| row.slot).collect();
        let expected: Vec<u16> = snapshot.rows().iter().map(|row| row.slot).collect();
        assert_eq!(slots, expected);
    }

    #[test]
    fn repeated_item_ids_stay_separate_rows() {
        let result = contents(&bank_with_rows(3), page(0, 10), ts("2026-01-01T12:00:00Z"));

        let quantities: Vec<(u32, u64)> = result
            .rows
            .iter()
            .map(|row| (row.item_id, row.quantity))
            .collect();
        assert_eq!(quantities, [(1, 1), (1, 2), (1, 3)]);
    }

    #[test]
    fn a_page_reports_offset_limit_total_and_truncation() {
        let snapshot = bank_with_rows(5);
        let now = ts("2026-01-01T12:00:00Z");

        let first = contents(&snapshot, page(0, 2), now);
        let middle = contents(&snapshot, page(2, 2), now);
        let last = contents(&snapshot, page(4, 2), now);

        assert_eq!(
            first.rows.iter().map(|row| row.slot).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(first.truncated);
        assert_eq!(
            middle.rows.iter().map(|row| row.slot).collect::<Vec<_>>(),
            [2, 3]
        );
        assert!(middle.truncated);
        assert_eq!(
            last.rows.iter().map(|row| row.slot).collect::<Vec<_>>(),
            [4]
        );
        assert!(!last.truncated);
        assert_eq!((last.offset, last.limit, last.total_rows), (4, 2, 5));
    }

    #[test]
    fn a_page_exactly_covering_the_rest_is_not_truncated() {
        let result = contents(&bank_with_rows(4), page(2, 2), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.rows.len(), 2);
        assert!(!result.truncated);
    }

    #[test]
    fn an_offset_past_the_end_is_an_empty_page_not_a_failure() {
        let result = contents(&bank_with_rows(3), page(10, 5), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.rows, []);
        assert_eq!(result.total_rows, 3);
        assert!(!result.truncated);
    }

    #[test]
    fn an_observed_empty_container_is_ok_with_no_rows() {
        let snapshot = parse_container_export(
            ContainerKind::Bank,
            br#"{"exported_at":"2026-01-01T12:00:00Z","kind":"bank","item_count":0,"items":[]}"#,
        )
        .unwrap();

        let result = contents(&snapshot, page(0, 100), ts("2026-01-01T12:00:00Z"));

        assert_eq!((result.status, result.total_rows), (ContentsStatus::Ok, 0));
        assert_eq!(result.rows, []);
        assert!(!result.truncated);
    }

    #[test]
    fn names_are_optional_and_bounded_without_changing_the_item_id() {
        let long = "x".repeat(super::super::export_output::MAX_NAME_CHARS + 5);
        let json = format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","kind":"bank","item_count":3,"items":[
                {{"slot":0,"id":7,"quantity":1}},
                {{"slot":1,"id":8,"quantity":1,"name":""}},
                {{"slot":2,"id":9,"quantity":1,"name":"{long}"}}]}}"#
        );
        let snapshot = parse_container_export(ContainerKind::Bank, json.as_bytes()).unwrap();

        let result = contents(&snapshot, page(0, 10), ts("2026-01-01T12:00:00Z"));

        assert_eq!(
            (
                result.rows[0].name.as_deref(),
                result.rows[0].name_truncated
            ),
            (None, false)
        );
        assert_eq!(
            (
                result.rows[1].name.as_deref(),
                result.rows[1].name_truncated
            ),
            (None, false)
        );
        assert!(result.rows[2].name_truncated);
        assert_eq!(result.rows[2].item_id, 9);
        assert_eq!(
            result.rows[2]
                .name
                .as_deref()
                .map(|name| name.chars().count()),
            Some(super::super::export_output::MAX_NAME_CHARS)
        );
    }

    #[test]
    fn an_item_count_disagreement_is_a_warning_and_rows_are_unchanged() {
        let snapshot = parse_container_export(
            ContainerKind::Bank,
            br#"{"exported_at":"2026-01-01T12:00:00Z","kind":"bank","item_count":9,
                "items":[{"slot":0,"id":1,"quantity":1}]}"#,
        )
        .unwrap();

        let result = contents(&snapshot, page(0, 10), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.warnings, [ContainerWarningOutput::ItemCountMismatch]);
        assert_eq!(result.warning_count, 1);
        assert!(!result.warnings_truncated);
    }

    #[test]
    fn a_future_export_time_gives_no_age() {
        let result = contents(&bank_with_rows(1), page(0, 10), ts("2025-12-31T00:00:00Z"));

        assert_eq!(result.observed.seconds_since_last_change, None);
        assert!(result.observed.exported_at_in_future);
    }

    #[test]
    fn omitted_paging_uses_the_defaults() {
        let page = PageInput {
            offset: None,
            limit: None,
        }
        .into_page();

        assert_eq!(
            page,
            Ok(Page {
                offset: 0,
                limit: DEFAULT_LIMIT
            })
        );
    }

    #[test]
    fn limits_outside_one_to_the_maximum_are_rejected() {
        for limit in [0, u32::try_from(MAX_LIMIT).unwrap() + 1, u32::MAX] {
            let input = PageInput {
                offset: None,
                limit: Some(limit),
            };
            assert!(input.into_page().is_err(), "limit {limit}");
        }
        let at_max = PageInput {
            offset: Some(3),
            limit: Some(u32::try_from(MAX_LIMIT).unwrap()),
        };
        assert_eq!(
            at_max.into_page(),
            Ok(Page {
                offset: 3,
                limit: MAX_LIMIT
            })
        );
    }

    #[test]
    fn failures_name_the_requested_file_and_never_a_path() {
        let failure = container_failure(&ExportReadError::FileNotFound, ContainerKind::Bank);

        assert!(failure.message.contains("bank.json"), "{}", failure.message);
    }
}
