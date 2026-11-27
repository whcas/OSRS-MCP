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

use crate::runelite::ExportReadError;

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

/// The first `limit` items mapped, and the total count.
pub(super) fn bounded<T, U>(items: &[T], limit: usize, map: impl Fn(&T) -> U) -> (Vec<U>, usize) {
    (items.iter().take(limit).map(map).collect(), items.len())
}

/// `name` cut to [`MAX_NAME_CHARS`] characters, and whether it was cut.
pub(super) fn bounded_name(name: &str) -> (String, bool) {
    match name.char_indices().nth(MAX_NAME_CHARS) {
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
