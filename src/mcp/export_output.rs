//! Output types and helpers shared by the account tools that read local
//! Character Exporter files: the exporter timestamp, the fixed failure
//! statuses and the bounding rules. Nothing here knows about a dataset.

use jiff::Timestamp;
use rmcp::{
    model::{CallToolResult, ContentBlock, ErrorData},
    schemars::JsonSchema,
    serde_json,
};
use serde::Serialize;

use super::OsrsMcpServer;
use crate::runelite::{ExportReadError, ExportSource, ExportedAt};

/// Longest display name returned, in characters. Names come from the export
/// and are untrusted.
pub(super) const MAX_NAME_CHARS: usize = 32;
/// Longest `plugin_version` returned, in bytes.
const MAX_VERSION_BYTES: usize = 16;

pub(super) const EXPORTED_AT_MEANING: &str = "Time of the last change to the exported data. It is not when \
this file was read and does not show whether the player is online.";

/// The exporter's timestamp, as written and normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExportedAtOutput {
    /// The value in the file, verbatim.
    pub text: String,
    /// The same instant in UTC, RFC 3339.
    pub utc: String,
}

/// When the data was last changed and when this server read it.
///
/// Shared by the tools that report one export file, so each states its own
/// file's time and none implies a snapshot consistent with another file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ObservationTime {
    /// When the exporter last changed the data.
    pub exported_at: ExportedAtOutput,
    /// What `exported_at` does and does not mean.
    pub exported_at_meaning: String,
    /// When this server read the file (its own clock, UTC, RFC 3339).
    pub read_at: String,
    /// Whole seconds from `exported_at` to `read_at`. Elapsed time since the
    /// last change, not staleness: an unchanged export is not rewritten. Null
    /// when `exported_at` is later than `read_at`.
    pub seconds_since_last_change: Option<u64>,
    /// True if `exported_at` is later than `read_at`, for example because of
    /// clock differences. The age is then unknown rather than zero.
    pub exported_at_in_future: bool,
    /// The exporter plugin version, null if the file has none.
    pub plugin_version: Option<String>,
    /// True if the file has a `plugin_version` that is not shown because it
    /// does not look like a version number.
    pub plugin_version_suppressed: bool,
}

impl ObservationTime {
    /// Describes an export timestamp read at `read_at`.
    pub(super) fn new(
        exported_at: &ExportedAt,
        plugin_version: Option<&str>,
        read_at: Timestamp,
    ) -> Self {
        let observed = exported_at.timestamp();
        let (plugin_version, plugin_version_suppressed) = shown_version(plugin_version);
        Self {
            exported_at: ExportedAtOutput {
                text: exported_at.text().to_owned(),
                utc: observed.to_string(),
            },
            exported_at_meaning: EXPORTED_AT_MEANING.to_owned(),
            read_at: read_at.to_string(),
            seconds_since_last_change: seconds_between(observed, read_at),
            exported_at_in_future: observed > read_at,
            plugin_version,
            plugin_version_suppressed,
        }
    }
}

/// Why no result was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum FailureStatus {
    /// `OSRS_MCP_RUNELITE_EXPORT_DIR` is unset or empty.
    NotConfigured,
    /// The configured path is missing or not a directory.
    DirectoryUnavailable,
    /// The directory has no file for the requested dataset.
    FileNotFound,
    /// The file exists but could not be read; see `reason`.
    Unreadable,
    /// The file was read but is not an acceptable export.
    InvalidExport,
}

/// More detail for [`FailureStatus::Unreadable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum UnreadableReason {
    /// `character.json` is not a regular file.
    NotARegularFile,
    /// The file is larger than the read limit.
    TooLarge,
    /// The operating system refused access.
    PermissionDenied,
    /// Another read failure.
    IoError,
}

/// A failed account-tool call, sent as the text of an error result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExportFailure {
    /// What went wrong.
    pub status: FailureStatus,
    /// Fixed explanatory text, safe to show.
    pub message: String,
    /// For `unreadable`: which kind of failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<UnreadableReason>,
    /// For `invalid_export`: true if the file ended unexpectedly. That can
    /// mean it was read during a write, but it is not proof of one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    /// For `invalid_export`: the importer's description of the problem. It
    /// never quotes the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl OsrsMcpServer {
    /// Reads one export file and builds the tool result from it.
    ///
    /// `read` runs on a blocking thread, since it is file I/O. `build` then
    /// receives the parsed export and the time of reading: a single clock
    /// sample taken after the read serves both `read_at` and the age. A read
    /// failure becomes the fixed failure for `file`.
    pub(super) async fn read_export<T, O>(
        &self,
        file: &'static str,
        read: impl FnOnce(&dyn ExportSource) -> Result<T, ExportReadError> + Send + 'static,
        build: impl FnOnce(&T, Timestamp) -> O + Send,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: Send + 'static,
        O: Serialize,
    {
        let source = self.export_source.clone();
        let read = tokio::task::spawn_blocking(move || read(source.as_ref()))
            .await
            .map_err(|_| ErrorData::internal_error("the export read task failed", None))?;
        let outcome = read
            .map(|export| build(&export, Timestamp::now()))
            .map_err(|error| failure_for(&error, file));
        into_result(outcome)
    }
}

/// Serialises the outcome. A failure is an error result whose text is the
/// failure as JSON, with no structured content, so it need not match the
/// success output schema.
pub(super) fn into_result<T: Serialize>(
    outcome: Result<T, ExportFailure>,
) -> Result<CallToolResult, ErrorData> {
    let internal = |_| ErrorData::internal_error("the result could not be serialised", None);
    match outcome {
        Ok(summary) => serde_json::to_value(summary)
            .map(CallToolResult::structured)
            .map_err(internal),
        Err(failure) => serde_json::to_string(&failure)
            .map(|text| CallToolResult::error(vec![ContentBlock::text(text)]))
            .map_err(internal),
    }
}

/// A validated window onto a list of results: how many to skip, then how many
/// to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Page {
    pub(super) offset: usize,
    pub(super) limit: usize,
}

impl Page {
    /// Validates the `offset` and `limit` arguments of a paged tool. A missing
    /// `limit` is `default_limit`; one outside `1..=max_limit` is rejected.
    pub(super) fn from_arguments(
        offset: Option<u32>,
        limit: Option<u32>,
        default_limit: usize,
        max_limit: usize,
    ) -> Result<Self, String> {
        let limit = match limit {
            None => default_limit,
            Some(limit) => usize::try_from(limit)
                .ok()
                .filter(|limit| (1..=max_limit).contains(limit))
                .ok_or_else(|| format!("limit must be between 1 and {max_limit}"))?,
        };
        let offset = offset
            .map_or(Ok(0), usize::try_from)
            .map_err(|_| "offset is too large".to_owned())?;
        Ok(Self { offset, limit })
    }

    /// The items of this page mapped, and whether more matching items follow
    /// it. `matching` is everything the caller's filter kept, so an offset
    /// past its end is an empty page, not an error.
    pub(super) fn select<T, U>(self, matching: &[T], map: impl Fn(&T) -> U) -> (Vec<U>, bool) {
        let shown: Vec<U> = matching
            .iter()
            .skip(self.offset)
            .take(self.limit)
            .map(map)
            .collect();
        let truncated = self.offset.saturating_add(shown.len()) < matching.len();
        (shown, truncated)
    }
}

/// The first `limit` items mapped, and the total count.
pub(super) fn bounded<T, U>(items: &[T], limit: usize, map: impl Fn(&T) -> U) -> (Vec<U>, usize) {
    (items.iter().take(limit).map(map).collect(), items.len())
}

/// `name` cut to [`MAX_NAME_CHARS`] characters, and whether it was cut.
pub(super) fn bounded_name(name: &str) -> (String, bool) {
    bounded_name_to(name, MAX_NAME_CHARS)
}

/// `name` cut to `max_chars` characters, and whether it was cut.
pub(super) fn bounded_name_to(name: &str, max_chars: usize) -> (String, bool) {
    match name.char_indices().nth(max_chars) {
        Some((end, _)) => (name[..end].to_owned(), true),
        None => (name.to_owned(), false),
    }
}

/// The version to show, and whether one was present but withheld.
///
/// Only text that looks like a version number is shown: the field is free
/// text in the file.
pub(super) fn shown_version(version: Option<&str>) -> (Option<String>, bool) {
    match version {
        None => (None, false),
        Some(text) if looks_like_version(text) => (Some(text.to_owned()), false),
        Some(_) => (None, true),
    }
}

fn looks_like_version(text: &str) -> bool {
    (1..=MAX_VERSION_BYTES).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
}

/// Whole seconds (rounded down) from `earlier` to `later`, or `None` if
/// `earlier` is later.
pub(super) fn seconds_between(earlier: Timestamp, later: Timestamp) -> Option<u64> {
    // Compared at full precision first, so a sub-second difference can never
    // give an age for a time that is also reported as being in the future.
    if earlier > later {
        return None;
    }
    u64::try_from(later.duration_since(earlier).as_secs()).ok()
}

/// Maps a read failure to a fixed status. No path, file content or operating
/// system message is used.
pub(super) fn failure_for(error: &ExportReadError, file: &str) -> ExportFailure {
    let failure = |status, message: &str| ExportFailure {
        status,
        message: message.to_owned(),
        reason: None,
        retryable: None,
        detail: None,
    };
    let unreadable = |reason, message: &str| ExportFailure {
        reason: Some(reason),
        ..failure(FailureStatus::Unreadable, message)
    };

    match error {
        ExportReadError::NotConfigured => failure(
            FailureStatus::NotConfigured,
            "No RuneLite export directory is configured. Set OSRS_MCP_RUNELITE_EXPORT_DIR to \
             the account's Character Exporter folder in the server configuration.",
        ),
        ExportReadError::DirectoryUnavailable => failure(
            FailureStatus::DirectoryUnavailable,
            "The configured export directory does not exist or is not a directory.",
        ),
        ExportReadError::FileNotFound => failure(
            FailureStatus::FileNotFound,
            &format!(
                "The export directory has no {file}. The exporter plugin may not have written \
                 it yet."
            ),
        ),
        ExportReadError::NotARegularFile => unreadable(
            UnreadableReason::NotARegularFile,
            &format!("{file} is not a regular file."),
        ),
        ExportReadError::TooLarge { .. } => unreadable(
            UnreadableReason::TooLarge,
            &format!("{file} is larger than the size this server reads."),
        ),
        ExportReadError::PermissionDenied => unreadable(
            UnreadableReason::PermissionDenied,
            &format!("The operating system refused access to {file}."),
        ),
        ExportReadError::Io { .. } => unreadable(
            UnreadableReason::IoError,
            &format!("{file} could not be read."),
        ),
        ExportReadError::Parse(parse_error) => {
            let message = if error.is_transient() {
                format!(
                    "{file} ended unexpectedly. This may be a write in progress, so reading \
                     again shortly may help, but that is not certain."
                )
            } else {
                format!("{file} is not an acceptable export.")
            };
            ExportFailure {
                retryable: Some(error.is_transient()),
                detail: Some(parse_error.to_string()),
                ..failure(FailureStatus::InvalidExport, &message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::{ServiceExt, model::CallToolRequestParams};

    use super::super::test_support::FakeMarket;
    use super::*;
    use crate::runelite::{CharacterExport, ContainerKind, ContainerSnapshot, QuestProgress};

    #[test]
    fn page_arguments_default_and_are_bounded_with_a_fixed_message() {
        assert_eq!(
            Page::from_arguments(None, None, 100, 200),
            Ok(Page {
                offset: 0,
                limit: 100
            })
        );
        assert_eq!(
            Page::from_arguments(Some(3), Some(200), 100, 200),
            Ok(Page {
                offset: 3,
                limit: 200
            })
        );
        for limit in [0, 201, u32::MAX] {
            assert_eq!(
                Page::from_arguments(None, Some(limit), 100, 200),
                Err("limit must be between 1 and 200".to_owned()),
                "limit {limit}"
            );
        }
    }

    #[test]
    fn a_page_reports_whether_matching_items_follow_it() {
        let items = [1, 2, 3, 4, 5];
        let page = |offset, limit| Page { offset, limit };

        assert_eq!(page(0, 2).select(&items, |n| *n), (vec![1, 2], true));
        assert_eq!(page(3, 2).select(&items, |n| *n), (vec![4, 5], false));
        assert_eq!(page(4, 10).select(&items, |n| *n), (vec![5], false));
        assert_eq!(page(9, 2).select(&items, |n| *n), (vec![], false));
    }

    /// An export source whose quest read is whatever the test supplies (other
    /// reads are not configured), so the
    /// adapter can be driven through failures a real directory cannot be made
    /// to produce on demand.
    #[derive(Debug)]
    struct ScriptedExports(fn() -> Result<QuestProgress, ExportReadError>);

    impl ExportSource for ScriptedExports {
        fn read_character(&self) -> Result<CharacterExport, ExportReadError> {
            Err(ExportReadError::NotConfigured)
        }

        fn read_container(&self, _: ContainerKind) -> Result<ContainerSnapshot, ExportReadError> {
            Err(ExportReadError::NotConfigured)
        }

        fn read_quests(&self) -> Result<QuestProgress, ExportReadError> {
            (self.0)()
        }
    }

    async fn quest_status_through(
        source: ScriptedExports,
    ) -> Result<CallToolResult, rmcp::ServiceError> {
        let server =
            OsrsMcpServer::with_market(Arc::new(FakeMarket::default())).with_export_source(source);
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let client = ().serve(client_io).await.expect("client should initialise");
        client
            .call_tool(CallToolRequestParams::new("quest_status"))
            .await
    }

    #[tokio::test]
    async fn an_operating_system_failure_is_a_fixed_unreadable_result() {
        let result = quest_status_through(ScriptedExports(|| {
            Err(ExportReadError::Io {
                kind: std::io::ErrorKind::Other,
            })
        }))
        .await
        .expect("a read failure is a tool result");

        assert_eq!(result.is_error, Some(true));
        let text = &result.content[0].as_text().expect("text").text;
        assert!(text.contains("unreadable"), "{text}");
    }

    #[tokio::test]
    async fn a_read_that_panics_is_an_internal_error_not_a_hung_call() {
        let outcome = quest_status_through(ScriptedExports(|| panic!("scripted failure"))).await;

        match outcome {
            Err(rmcp::ServiceError::McpError(error)) => {
                assert_eq!(error.message, "the export read task failed");
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }
}
