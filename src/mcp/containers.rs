//! The `bank_contents`, `equipment_contents` and `inventory_contents` tools:
//! item rows from the local Character Exporter files.
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
use crate::domain::ItemId;
use crate::runelite::{
    ContainerKind, ContainerRow, ContainerSnapshot, ContainerWarning, ExportReadError,
};

/// Rows returned when `limit` is not given.
const DEFAULT_LIMIT: usize = 100;
/// Most rows one call can return.
const MAX_LIMIT: usize = 200;
/// Slots in the game's inventory. The shared parser accepts far more rows than
/// this, so the inventory tool bounds its own output.
const INVENTORY_SLOTS: usize = 28;
/// Most warnings returned.
const MAX_WARNINGS: usize = 64;

/// Most item IDs one `bank_contents` call can ask for.
const MAX_ITEM_IDS: usize = 50;

/// Arguments of the `bank_contents` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct BankInput {
    /// Rows to skip, in the order the file lists them (after `item_ids` is
    /// applied, if given). Defaults to 0.
    offset: Option<u32>,
    /// Most rows to return, 1 to 200. Defaults to 100.
    limit: Option<u32>,
    /// Only rows holding one of these exact item IDs, 1 to 50 IDs. The whole
    /// bank is searched before `offset` and `limit` apply to the matching
    /// rows. Requested IDs with no row anywhere in the bank are listed in
    /// `item_filter.unmatched_item_ids`. Omit to browse every row.
    item_ids: Option<Vec<u32>>,
}

/// A validated page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Page {
    offset: usize,
    limit: usize,
}

/// A validated request: which rows, then which page of them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Query {
    page: Page,
    /// Requested IDs without repeats, in request order. `None` means no filter.
    item_ids: Option<Vec<ItemId>>,
}

impl BankInput {
    fn into_query(self) -> Result<Query, String> {
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
        let item_ids = self.item_ids.map(distinct_item_ids).transpose()?;
        Ok(Query {
            page: Page { offset, limit },
            item_ids,
        })
    }
}

/// The requested IDs without repeats, in request order. A repeat is harmless
/// and ignored; an empty or oversized list is not.
fn distinct_item_ids(requested: Vec<u32>) -> Result<Vec<ItemId>, String> {
    if !(1..=MAX_ITEM_IDS).contains(&requested.len()) {
        return Err(format!(
            "item_ids must list between 1 and {MAX_ITEM_IDS} item ids"
        ));
    }
    let mut ids: Vec<ItemId> = Vec::with_capacity(requested.len());
    for id in requested.into_iter().map(ItemId) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
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

/// The outcome of an `item_ids` filter, computed over the whole snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ItemFilterOutput {
    /// The requested item IDs, without repeats, in request order.
    pub requested_item_ids: Vec<u32>,
    /// Requested IDs with no row anywhere in the snapshot. An ID that matches
    /// a row on another page is not listed here.
    pub unmatched_item_ids: Vec<u32>,
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
    /// The requested page of matching rows, in the order the file lists them.
    /// Repeated item IDs are separate rows and are not merged.
    pub rows: Vec<ContainerRowOutput>,
    /// Rows the page is taken from: every row in the file, or with an
    /// `item_ids` filter every row holding a requested ID, counted before
    /// paging. An empty container reports 0 with status `ok`.
    pub total_rows: usize,
    /// Rows in the whole file, whatever the filter.
    pub snapshot_rows: usize,
    /// Present with an `item_ids` filter, otherwise null.
    pub item_filter: Option<ItemFilterOutput>,
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
                       the file on every call. Browse with offset and limit (default 100, at \
                       most 200 rows), or pass item_ids (1 to 50 exact item ids) to get only \
                       the rows holding those items: the whole bank is searched first, then \
                       offset and limit apply to the matching rows, every matching slot is \
                       returned in file order, and item_filter.unmatched_item_ids lists \
                       requested ids that appear nowhere in the bank. total_rows counts rows \
                       before paging and snapshot_rows counts the whole bank. The timestamp is \
                       the last change to the bank file, not the time of reading, and the \
                       exporter only writes the bank after it has been opened, so the file can \
                       be sessions old. A missing file is reported as file_not_found, never as \
                       an empty bank. Does not price items or total their value, and is not \
                       consistent with any other export file. Resolve item names with \
                       item_search. Failures report a status such as not_configured or \
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
        Parameters(input): Parameters<BankInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = input
            .into_query()
            .map_err(|message| ErrorData::invalid_params(message, None))?;
        self.container_result(ContainerKind::Bank, query).await
    }

    /// Reports equipped items from the local Character Exporter file.
    #[tool(
        name = "equipment_contents",
        description = "Read the player's equipped items from the local RuneLite Character \
                       Exporter file configured for this server: item ids, quantities and \
                       optional display names per equipment slot, with the exporter timestamp \
                       and import warnings. Reads the file on every call. An observed empty \
                       equipment file is a valid result with no rows; a missing file is \
                       reported as file_not_found. The timestamp is the last change to the \
                       equipment file, not the time of reading, and the file is not consistent \
                       with any other export file. Does not price items or total their value. \
                       Failures report a status such as not_configured or file_not_found.",
        annotations(
            title = "Equipment contents",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        output_schema = schema_for_output::<ContainerContents>()
    )]
    async fn equipment_contents(&self) -> Result<CallToolResult, ErrorData> {
        // A character has at most 14 equipment slots, so one page holds them all.
        let query = Query {
            page: Page {
                offset: 0,
                limit: MAX_LIMIT,
            },
            item_ids: None,
        };
        self.container_result(ContainerKind::Equipment, query).await
    }

    /// Reports inventory rows from the local Character Exporter file.
    #[tool(
        name = "inventory_contents",
        description = "Read the player's inventory from the local RuneLite Character Exporter \
                       file configured for this server: item ids, quantities and optional \
                       display names per slot, with the exporter timestamp and import \
                       warnings. Reads the file on every call. At most 28 rows are returned, \
                       the size of the game's inventory; if the file lists more, truncated is \
                       true and total_rows and snapshot_rows give the full count. An observed \
                       empty inventory file is a valid result with no rows; a missing file is \
                       reported as file_not_found. The exporter throttles inventory writes and \
                       does not reschedule a dropped one, so the file can be one change behind, \
                       and the timestamp is the last change to the file, not the time of \
                       reading. It does not show the current inventory and is not consistent \
                       with any other export file. Does not price items or total their value. \
                       Failures report a status such as not_configured or file_not_found.",
        annotations(
            title = "Inventory contents",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        output_schema = schema_for_output::<ContainerContents>()
    )]
    async fn inventory_contents(&self) -> Result<CallToolResult, ErrorData> {
        let query = Query {
            page: Page {
                offset: 0,
                limit: INVENTORY_SLOTS,
            },
            item_ids: None,
        };
        self.container_result(ContainerKind::Inventory, query).await
    }
}

impl OsrsMcpServer {
    /// Reads one container off the async executor and builds the tool result.
    async fn container_result(
        &self,
        kind: ContainerKind,
        query: Query,
    ) -> Result<CallToolResult, ErrorData> {
        let source = self.export_source.clone();
        let read = tokio::task::spawn_blocking(move || source.read_container(kind))
            .await
            .map_err(|_| ErrorData::internal_error("the export read task failed", None))?;

        // One clock sample, taken after the read, serves both `read_at` and the age.
        let outcome = read
            .map(|snapshot| contents(&snapshot, &query, Timestamp::now()))
            .map_err(|error| container_failure(&error, kind));
        into_result(outcome)
    }
}

/// The fixed failure for `kind`, naming its file.
fn container_failure(error: &ExportReadError, kind: ContainerKind) -> ExportFailure {
    failure_for(error, crate::runelite::container_file_name(kind))
}

/// Builds the response for a parsed snapshot, read at `read_at`.
fn contents(snapshot: &ContainerSnapshot, query: &Query, read_at: Timestamp) -> ContainerContents {
    let (warnings, warning_count) = bounded(snapshot.warnings(), MAX_WARNINGS, warning_output);
    // Filtering looks at every row; the page is taken afterwards, so a match
    // on a later page can never be reported as absent.
    let matching = matching_rows(snapshot.rows(), query.item_ids.as_deref());
    let page = query.page;
    let shown: Vec<ContainerRowOutput> = matching
        .iter()
        .skip(page.offset)
        .take(page.limit)
        .map(|row| row_output(row))
        .collect();

    ContainerContents {
        status: ContentsStatus::Ok,
        container: container_output(snapshot.kind()),
        observed: ObservationTime::new(snapshot.exported_at(), snapshot.plugin_version(), read_at),
        truncated: page.offset.saturating_add(shown.len()) < matching.len(),
        rows: shown,
        total_rows: matching.len(),
        snapshot_rows: snapshot.rows().len(),
        item_filter: query
            .item_ids
            .as_deref()
            .map(|requested| item_filter_output(requested, &matching)),
        offset: page.offset,
        limit: page.limit,
        warnings_truncated: warning_count > warnings.len(),
        warnings,
        warning_count,
    }
}

/// The rows to page through, in file order: all of them, or those holding a
/// requested item ID.
fn matching_rows<'a>(
    rows: &'a [ContainerRow],
    item_ids: Option<&[ItemId]>,
) -> Vec<&'a ContainerRow> {
    rows.iter()
        .filter(|row| item_ids.is_none_or(|ids| ids.contains(&row.item_id)))
        .collect()
}

fn item_filter_output(requested: &[ItemId], matching: &[&ContainerRow]) -> ItemFilterOutput {
    let unmatched = requested
        .iter()
        .filter(|id| !matching.iter().any(|row| row.item_id == **id));
    ItemFilterOutput {
        requested_item_ids: requested.iter().map(|id| id.0).collect(),
        unmatched_item_ids: unmatched.map(|id| id.0).collect(),
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

    fn page(offset: usize, limit: usize) -> Query {
        Query {
            page: Page { offset, limit },
            item_ids: None,
        }
    }

    #[test]
    fn a_synthetic_bank_is_reported_with_both_timestamps_and_in_file_order() {
        let snapshot = parse_container_export(ContainerKind::Bank, BANK).unwrap();

        let result = contents(&snapshot, &page(0, 100), ts("2026-01-01T12:01:00Z"));

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
        let result = contents(&bank_with_rows(3), &page(0, 10), ts("2026-01-01T12:00:00Z"));

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

        let first = contents(&snapshot, &page(0, 2), now);
        let middle = contents(&snapshot, &page(2, 2), now);
        let last = contents(&snapshot, &page(4, 2), now);

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
        let result = contents(&bank_with_rows(4), &page(2, 2), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.rows.len(), 2);
        assert!(!result.truncated);
    }

    #[test]
    fn an_offset_past_the_end_is_an_empty_page_not_a_failure() {
        let result = contents(&bank_with_rows(3), &page(10, 5), ts("2026-01-01T12:00:00Z"));

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

        let result = contents(&snapshot, &page(0, 100), ts("2026-01-01T12:00:00Z"));

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

        let result = contents(&snapshot, &page(0, 10), ts("2026-01-01T12:00:00Z"));

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

        let result = contents(&snapshot, &page(0, 10), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.warnings, [ContainerWarningOutput::ItemCountMismatch]);
        assert_eq!(result.warning_count, 1);
        assert!(!result.warnings_truncated);
    }

    #[test]
    fn a_future_export_time_gives_no_age() {
        let result = contents(&bank_with_rows(1), &page(0, 10), ts("2025-12-31T00:00:00Z"));

        assert_eq!(result.observed.seconds_since_last_change, None);
        assert!(result.observed.exported_at_in_future);
    }

    fn input(offset: Option<u32>, limit: Option<u32>, item_ids: Option<Vec<u32>>) -> BankInput {
        BankInput {
            offset,
            limit,
            item_ids,
        }
    }

    #[test]
    fn omitted_arguments_browse_the_first_default_page() {
        let query = input(None, None, None).into_query();

        assert_eq!(
            query,
            Ok(Query {
                page: Page {
                    offset: 0,
                    limit: DEFAULT_LIMIT
                },
                item_ids: None
            })
        );
    }

    #[test]
    fn limits_outside_one_to_the_maximum_are_rejected() {
        let over = u32::try_from(MAX_LIMIT).unwrap() + 1;
        for limit in [0, over, u32::MAX] {
            assert!(
                input(None, Some(limit), None).into_query().is_err(),
                "limit {limit}"
            );
        }
        let at_max = input(Some(3), Some(over - 1), None).into_query();
        assert_eq!(
            at_max,
            Ok(Query {
                page: Page {
                    offset: 3,
                    limit: MAX_LIMIT
                },
                item_ids: None
            })
        );
    }

    #[test]
    fn item_id_lists_are_bounded_and_repeats_are_ignored() {
        assert!(input(None, None, Some(vec![])).into_query().is_err());
        let too_many: Vec<u32> = (1..=u32::try_from(MAX_ITEM_IDS).unwrap() + 1).collect();
        assert!(input(None, None, Some(too_many)).into_query().is_err());
        let at_max: Vec<u32> = (1..=u32::try_from(MAX_ITEM_IDS).unwrap()).collect();
        assert!(input(None, None, Some(at_max)).into_query().is_ok());

        let query = input(None, None, Some(vec![7, 3, 7, 3, 9]))
            .into_query()
            .unwrap();

        assert_eq!(query.item_ids, Some(vec![ItemId(7), ItemId(3), ItemId(9)]));
    }

    /// A bank of items 10, 20, 10, 30, 20, 10 in slots 0 to 5.
    fn mixed_bank() -> ContainerSnapshot {
        let rows: Vec<String> = [10, 20, 10, 30, 20, 10]
            .iter()
            .enumerate()
            .map(|(slot, id)| format!(r#"{{"slot":{slot},"id":{id},"quantity":1}}"#))
            .collect();
        let json = format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","kind":"bank","item_count":6,"items":[{}]}}"#,
            rows.join(",")
        );
        parse_container_export(ContainerKind::Bank, json.as_bytes()).unwrap()
    }

    fn filtered(ids: &[u32], offset: usize, limit: usize) -> ContainerContents {
        let query = Query {
            page: Page { offset, limit },
            item_ids: Some(ids.iter().copied().map(ItemId).collect()),
        };
        contents(&mixed_bank(), &query, ts("2026-01-01T12:00:00Z"))
    }

    #[test]
    fn a_filter_keeps_every_matching_slot_in_file_order() {
        let result = filtered(&[10, 30], 0, 100);

        let slots: Vec<(u16, u32)> = result
            .rows
            .iter()
            .map(|row| (row.slot, row.item_id))
            .collect();
        assert_eq!(slots, [(0, 10), (2, 10), (3, 30), (5, 10)]);
        assert_eq!((result.total_rows, result.snapshot_rows), (4, 6));
        assert!(!result.truncated);
    }

    #[test]
    fn filtered_paging_applies_to_the_matching_rows() {
        let result = filtered(&[10], 1, 1);

        assert_eq!(
            result.rows.iter().map(|row| row.slot).collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(result.total_rows, 3);
        assert!(result.truncated);
    }

    #[test]
    fn an_id_matching_only_beyond_the_page_is_not_reported_absent() {
        // Item 30 sits in slot 3, after the one row (item 20, slot 1) this page shows.
        let result = filtered(&[30, 20], 0, 1);

        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.item_id)
                .collect::<Vec<_>>(),
            [20]
        );
        assert!(result.truncated);
        let filter = result.item_filter.unwrap();
        assert_eq!(filter.unmatched_item_ids, Vec::<u32>::new());
    }

    #[test]
    fn requested_ids_with_no_row_anywhere_are_reported_in_request_order() {
        let result = filtered(&[99, 30, 98], 0, 100);

        let filter = result.item_filter.unwrap();
        assert_eq!(filter.requested_item_ids, [99, 30, 98]);
        assert_eq!(filter.unmatched_item_ids, [99, 98]);
    }

    #[test]
    fn a_filter_matching_nothing_is_ok_not_an_empty_bank() {
        let result = filtered(&[99], 0, 100);

        assert_eq!(result.rows, []);
        assert_eq!((result.total_rows, result.snapshot_rows), (0, 6));
        assert_eq!(result.item_filter.unwrap().unmatched_item_ids, [99]);
    }

    #[test]
    fn an_unfiltered_response_has_no_filter_section() {
        let result = contents(&mixed_bank(), &page(0, 100), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.item_filter, None);
        assert_eq!(result.total_rows, result.snapshot_rows);
    }

    #[test]
    fn failures_name_the_requested_file_and_never_a_path() {
        let failure = container_failure(&ExportReadError::FileNotFound, ContainerKind::Bank);

        assert!(failure.message.contains("bank.json"), "{}", failure.message);
    }

    #[test]
    fn equipment_rows_report_slots_and_unknown_slots_warn() {
        let snapshot = parse_container_export(
            ContainerKind::Equipment,
            br#"{"exported_at":"2026-01-01T12:00:00Z","kind":"equipment","item_count":2,
                "items":[{"slot":3,"id":4151,"quantity":1},{"slot":99,"id":1,"quantity":1}]}"#,
        )
        .unwrap();

        let result = contents(&snapshot, &page(0, MAX_LIMIT), ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.container, ContainerOutput::Equipment);
        assert_eq!(
            result.rows.iter().map(|row| row.slot).collect::<Vec<_>>(),
            [3, 99]
        );
        assert_eq!(
            result.warnings,
            [ContainerWarningOutput::UnknownEquipmentSlot { row: 2 }]
        );
    }

    #[test]
    fn equipment_failures_name_the_equipment_file() {
        let failure = container_failure(&ExportReadError::FileNotFound, ContainerKind::Equipment);

        assert!(
            failure.message.contains("equipment.json"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn inventory_output_is_bounded_to_the_game_inventory_and_says_so() {
        let snapshot = bank_with_rows(INVENTORY_SLOTS + 1);
        let query = Query {
            page: Page {
                offset: 0,
                limit: INVENTORY_SLOTS,
            },
            item_ids: None,
        };

        let result = contents(&snapshot, &query, ts("2026-01-01T12:00:00Z"));

        assert_eq!(result.rows.len(), INVENTORY_SLOTS);
        assert!(result.truncated);
        assert_eq!(result.total_rows, INVENTORY_SLOTS + 1);
        assert_eq!(result.snapshot_rows, INVENTORY_SLOTS + 1);
    }

    #[test]
    fn exactly_a_full_inventory_is_not_truncated() {
        let query = Query {
            page: Page {
                offset: 0,
                limit: INVENTORY_SLOTS,
            },
            item_ids: None,
        };

        let result = contents(
            &bank_with_rows(INVENTORY_SLOTS),
            &query,
            ts("2026-01-01T12:00:00Z"),
        );

        assert_eq!(result.rows.len(), INVENTORY_SLOTS);
        assert!(!result.truncated);
    }

    #[test]
    fn inventory_failures_name_the_inventory_file() {
        let failure = container_failure(&ExportReadError::FileNotFound, ContainerKind::Inventory);

        assert!(
            failure.message.contains("inventory.json"),
            "{}",
            failure.message
        );
    }
}
