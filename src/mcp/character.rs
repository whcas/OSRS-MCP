//! The `character_summary` tool: skill levels from the local Character
//! Exporter file.
//!
//! This module only adapts. Reading the file is [`LocalExportSource`] and
//! parsing is [`parse_character_export`](crate::runelite::parse_character_export);
//! here a parsed export becomes a bounded response and a read failure becomes a
//! fixed, safe status. Nothing from the export beyond skill names, levels,
//! experience, the exporter timestamp and the plugin version is returned: no
//! account name, world, session id, path or raw JSON, and no account mode or
//! activity is inferred.

use jiff::Timestamp;
use rmcp::{
    handler::server::tool::schema_for_output,
    model::{CallToolResult, ContentBlock, ErrorData},
    schemars::JsonSchema,
    serde_json, tool, tool_router,
};
use serde::Serialize;

use super::OsrsMcpServer;
use crate::runelite::{CharacterExport, ExportReadError, Skill, SkillRecord, Warning};

/// Most skills returned. Real exports have about two dozen.
const MAX_SKILLS: usize = 64;
/// Most warnings returned.
const MAX_WARNINGS: usize = 64;
/// Longest skill name returned, in characters. Names the importer does not
/// recognise come from the file and are untrusted.
const MAX_NAME_CHARS: usize = 32;
/// Longest `plugin_version` returned, in bytes.
const MAX_VERSION_BYTES: usize = 16;

const EXPORTED_AT_MEANING: &str = "Time of the last change to the exported data. It is not when \
this file was read and does not show whether the player is online.";

/// Success marker, so a client can tell results apart by `status` alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    /// The export was read and imported. Warnings may be present.
    Ok,
}

/// The exporter's timestamp, as written and normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExportedAtOutput {
    /// The value in the file, verbatim.
    pub text: String,
    /// The same instant in UTC, RFC 3339.
    pub utc: String,
}

/// One skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SkillOutput {
    /// The `RuneLite` skill name. For a skill this server does not recognise
    /// it is the name from the file, cut to 32 characters if longer (see
    /// `name_truncated`).
    pub name: String,
    /// True if `name` was cut short and is therefore not the skill's full name.
    pub name_truncated: bool,
    /// False for a skill name this server does not recognise.
    pub recognised: bool,
    /// Real (unboosted) level, 1 to 99.
    pub real_level: u8,
    /// Total experience points, 0 to 200,000,000.
    pub experience: u32,
}

/// A non-fatal finding from the import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WarningOutput {
    /// The recorded level differs from the level the experience implies.
    LevelExperienceMismatch {
        /// The skill concerned (always a recognised skill).
        skill: String,
        /// The level in the export.
        recorded_level: u8,
        /// The level implied by the experience table.
        implied_level: u8,
    },
    /// A skill name this server does not recognise; it is listed in `skills`.
    UnknownSkill {
        /// Its 1-based position among the exported skills.
        position: usize,
    },
}

/// Successful result of `character_summary`.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat wire contract: each flag answers a distinct question for the client, and \
              grouping them would change the approved response schema"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CharacterSummary {
    /// Always `ok`.
    pub status: SummaryStatus,
    /// When the exporter last changed the data.
    pub exported_at: ExportedAtOutput,
    /// What `exported_at` does and does not mean.
    pub exported_at_meaning: String,
    /// When this server read the file (its own clock, UTC, RFC 3339).
    pub read_at: String,
    /// Whole seconds from `exported_at` to `read_at`. Elapsed time since the
    /// last change, not staleness: an unchanged export is not rewritten. Null
    /// when `exported_at` is later than `read_at` (see `exported_at_in_future`).
    pub seconds_since_last_change: Option<u64>,
    /// True if `exported_at` is later than `read_at`, for example because of
    /// clock differences. The age is then unknown rather than zero.
    pub exported_at_in_future: bool,
    /// The exporter plugin version, null if the file has none.
    pub plugin_version: Option<String>,
    /// True if the file has a `plugin_version` that is not shown because it
    /// does not look like a version number.
    pub plugin_version_suppressed: bool,
    /// Skills in the order the file lists them, at most 64.
    pub skills: Vec<SkillOutput>,
    /// How many skills the export contains. Greater than `skills.len()` only
    /// if `skills_truncated`.
    pub skill_count: usize,
    /// True if `skills` omits some skills.
    pub skills_truncated: bool,
    /// Non-fatal findings, at most 64.
    pub warnings: Vec<WarningOutput>,
    /// How many warnings the import produced.
    pub warning_count: usize,
    /// True if `warnings` omits some warnings.
    pub warnings_truncated: bool,
}

/// Why no summary was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum FailureStatus {
    /// `OSRS_MCP_RUNELITE_EXPORT_DIR` is unset or empty.
    NotConfigured,
    /// The configured path is missing or not a directory.
    DirectoryUnavailable,
    /// The directory has no `character.json`.
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

/// A failed `character_summary` call, sent as the text of an error result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CharacterSummaryFailure {
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

#[tool_router(router = character_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Reports skill levels and experience from the local Character Exporter file.
    #[tool(
        name = "character_summary",
        description = "Read the player's skill levels and experience from the local RuneLite \
                       Character Exporter file configured for this server, with the exporter \
                       timestamp and any import warnings. Reads the file on every call. The \
                       timestamp is the last change to the data, not the time of reading or \
                       proof that the player is online. Reports skills only: it does not \
                       identify the account or its game mode, and returns no raw file content. \
                       Failures report a status such as not_configured or file_not_found.",
        annotations(
            title = "Character summary",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        output_schema = schema_for_output::<CharacterSummary>()
    )]
    async fn character_summary(&self) -> Result<CallToolResult, ErrorData> {
        // The read is blocking file I/O, so it stays off the async executor.
        let source = self.export_source.clone();
        let read = tokio::task::spawn_blocking(move || source.read_character())
            .await
            .map_err(|_| ErrorData::internal_error("the export read task failed", None))?;

        // One clock sample, taken after the read, serves both `read_at` and the age.
        let outcome = read
            .map(|export| summarise(&export, Timestamp::now()))
            .map_err(|error| failure_for(&error));
        into_result(outcome)
    }
}

/// Serialises the outcome. A failure is an error result whose text is the
/// failure as JSON, with no structured content, so it need not match the
/// success output schema.
fn into_result(
    outcome: Result<CharacterSummary, CharacterSummaryFailure>,
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

/// Builds the response for a parsed export, read at `read_at`.
fn summarise(export: &CharacterExport, read_at: Timestamp) -> CharacterSummary {
    let exported_at = export.exported_at().timestamp();
    let (plugin_version, plugin_version_suppressed) = shown_version(export.plugin_version());
    let (warnings, warning_count) = bounded(export.warnings(), MAX_WARNINGS, warning_output);
    let (skills, skill_count) = bounded(export.skills(), MAX_SKILLS, skill_output);

    CharacterSummary {
        status: SummaryStatus::Ok,
        exported_at: ExportedAtOutput {
            text: export.exported_at().text().to_owned(),
            utc: exported_at.to_string(),
        },
        exported_at_meaning: EXPORTED_AT_MEANING.to_owned(),
        read_at: read_at.to_string(),
        seconds_since_last_change: seconds_between(exported_at, read_at),
        exported_at_in_future: exported_at > read_at,
        plugin_version,
        plugin_version_suppressed,
        skills_truncated: skill_count > skills.len(),
        skills,
        skill_count,
        warnings_truncated: warning_count > warnings.len(),
        warnings,
        warning_count,
    }
}

/// The first `limit` items mapped, and the total count.
fn bounded<T, U>(items: &[T], limit: usize, map: impl Fn(&T) -> U) -> (Vec<U>, usize) {
    (items.iter().take(limit).map(map).collect(), items.len())
}

fn skill_output(record: &SkillRecord) -> SkillOutput {
    let (name, name_truncated) = bounded_name(record.skill.name());
    SkillOutput {
        name,
        name_truncated,
        recognised: !matches!(record.skill, Skill::Unknown(_)),
        real_level: record.real_level,
        experience: record.experience,
    }
}

fn warning_output(warning: &Warning) -> WarningOutput {
    match warning {
        Warning::LevelExperienceMismatch {
            skill,
            recorded_level,
            implied_level,
        } => WarningOutput::LevelExperienceMismatch {
            skill: bounded_name(skill.name()).0,
            recorded_level: *recorded_level,
            implied_level: *implied_level,
        },
        Warning::UnknownSkill { position } => WarningOutput::UnknownSkill {
            position: *position,
        },
    }
}

/// `name` cut to [`MAX_NAME_CHARS`] characters, and whether it was cut.
fn bounded_name(name: &str) -> (String, bool) {
    match name.char_indices().nth(MAX_NAME_CHARS) {
        Some((end, _)) => (name[..end].to_owned(), true),
        None => (name.to_owned(), false),
    }
}

/// The version to show, and whether one was present but withheld.
///
/// Only text that looks like a version number is shown: the field is free
/// text in the file.
fn shown_version(version: Option<&str>) -> (Option<String>, bool) {
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
fn seconds_between(earlier: Timestamp, later: Timestamp) -> Option<u64> {
    // Compared at full precision first, so a sub-second difference can never
    // give an age for a time that is also reported as being in the future.
    if earlier > later {
        return None;
    }
    u64::try_from(later.duration_since(earlier).as_secs()).ok()
}

/// Maps a read failure to a fixed status. No path, file content or operating
/// system message is used.
fn failure_for(error: &ExportReadError) -> CharacterSummaryFailure {
    let failure = |status, message: &str| CharacterSummaryFailure {
        status,
        message: message.to_owned(),
        reason: None,
        retryable: None,
        detail: None,
    };
    let unreadable = |reason, message: &str| CharacterSummaryFailure {
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
            "The export directory has no character.json. The exporter plugin may not have \
             written it yet.",
        ),
        ExportReadError::NotARegularFile => unreadable(
            UnreadableReason::NotARegularFile,
            "character.json is not a regular file.",
        ),
        ExportReadError::TooLarge { .. } => unreadable(
            UnreadableReason::TooLarge,
            "character.json is larger than the size this server reads.",
        ),
        ExportReadError::PermissionDenied => unreadable(
            UnreadableReason::PermissionDenied,
            "The operating system refused access to character.json.",
        ),
        ExportReadError::Io { .. } => unreadable(
            UnreadableReason::IoError,
            "character.json could not be read.",
        ),
        ExportReadError::Parse(parse_error) => CharacterSummaryFailure {
            retryable: Some(error.is_transient()),
            detail: Some(parse_error.to_string()),
            ..failure(
                FailureStatus::InvalidExport,
                if error.is_transient() {
                    "character.json ended unexpectedly. This may be a write in progress, so \
                     reading again shortly may help, but that is not certain."
                } else {
                    "character.json is not an acceptable export."
                },
            )
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::runelite::{CharacterParseError, parse_character_export};

    const SYNTHETIC_EXPORT: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/character.synthetic.json");

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn export_from(json: &str) -> CharacterExport {
        parse_character_export(json.as_bytes()).unwrap()
    }

    /// An export of `count` skills named `Unknown0`, `Unknown1`, ...
    fn export_with_unknown_skills(count: usize) -> CharacterExport {
        let stats: Vec<String> = (0..count)
            .map(|index| format!(r#""Unknown{index}":{{"real_level":1,"experience":0}}"#))
            .collect();
        export_from(&format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","stats":{{{}}}}}"#,
            stats.join(",")
        ))
    }

    fn synthetic_summary(read_at: &str) -> CharacterSummary {
        let export = parse_character_export(SYNTHETIC_EXPORT).unwrap();
        summarise(&export, ts(read_at))
    }

    #[test]
    fn a_synthetic_export_is_summarised_with_both_timestamps() {
        let summary = synthetic_summary("2026-01-01T12:01:00Z");

        assert_eq!(summary.status, SummaryStatus::Ok);
        assert_eq!(summary.exported_at.text, "2026-01-01T12:00:00.123456Z");
        assert_eq!(summary.exported_at.utc, "2026-01-01T12:00:00.123456Z");
        assert_eq!(summary.read_at, "2026-01-01T12:01:00Z");
        assert_eq!(summary.seconds_since_last_change, Some(59));
        assert!(!summary.exported_at_in_future);
        assert_eq!(summary.plugin_version.as_deref(), Some("0.6.0"));
        assert!(!summary.plugin_version_suppressed);
        assert_eq!(summary.skill_count, 24);
        assert_eq!(summary.skills.len(), 24);
        assert!(!summary.skills_truncated);
        assert!(summary.skills.iter().all(|skill| skill.recognised));
    }

    #[test]
    fn skills_keep_the_file_order_and_report_levels_and_experience() {
        let export = export_from(
            r#"{"exported_at":"2026-01-01T12:00:00Z","stats":{
                "Hitpoints":{"real_level":10,"experience":1154},
                "Attack":{"real_level":1,"experience":0}}}"#,
        );

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        let listed: Vec<(&str, u8, u32)> = summary
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.real_level, skill.experience))
            .collect();
        assert_eq!(listed, [("Hitpoints", 10, 1154), ("Attack", 1, 0)]);
    }

    #[test]
    fn the_age_is_zero_when_read_in_the_same_second() {
        let summary = synthetic_summary("2026-01-01T12:00:00.5Z");

        assert_eq!(summary.seconds_since_last_change, Some(0));
        assert!(!summary.exported_at_in_future);
    }

    #[test]
    fn a_future_export_time_has_no_age_and_is_flagged() {
        let summary = synthetic_summary("2025-12-31T12:00:00Z");

        assert_eq!(summary.seconds_since_last_change, None);
        assert!(summary.exported_at_in_future);
    }

    #[test]
    fn a_sub_second_future_export_time_is_flagged_and_has_no_age() {
        // Exported at .123456, read at .1: the same whole second.
        let summary = synthetic_summary("2026-01-01T12:00:00.1Z");

        assert_eq!(summary.seconds_since_last_change, None);
        assert!(summary.exported_at_in_future);
    }

    #[test]
    fn an_old_export_is_not_called_stale() {
        // 365 days less the fraction of a second in the exported time.
        let summary = synthetic_summary("2027-01-01T12:00:00Z");

        assert_eq!(summary.seconds_since_last_change, Some(31_535_999));
        assert!(!summary.exported_at_meaning.to_lowercase().contains("stale"));
    }

    #[test]
    fn skills_beyond_the_limit_are_omitted_and_flagged() {
        let export = export_with_unknown_skills(MAX_SKILLS + 6);

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        assert_eq!(summary.skills.len(), MAX_SKILLS);
        assert_eq!(summary.skill_count, MAX_SKILLS + 6);
        assert!(summary.skills_truncated);
    }

    #[test]
    fn warnings_beyond_the_limit_are_omitted_and_flagged() {
        // Each unknown skill produces one warning.
        let export = export_with_unknown_skills(MAX_WARNINGS + 3);

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        assert_eq!(summary.warnings.len(), MAX_WARNINGS);
        assert_eq!(summary.warning_count, MAX_WARNINGS + 3);
        assert!(summary.warnings_truncated);
    }

    #[test]
    fn nothing_is_flagged_at_exactly_the_limits() {
        let export = export_with_unknown_skills(MAX_SKILLS);

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        assert!(!summary.skills_truncated);
        assert!(!summary.warnings_truncated);
    }

    #[test]
    fn an_unrecognised_skill_is_kept_and_marked_as_such() {
        let export = export_from(
            r#"{"exported_at":"2026-01-01T12:00:00Z","stats":{
                "Attack":{"real_level":1,"experience":0},
                "Sailing2":{"real_level":3,"experience":200}}}"#,
        );

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        assert!(summary.skills[0].recognised);
        assert!(!summary.skills[1].recognised);
        assert_eq!(summary.skills[1].name, "Sailing2");
        assert_eq!(
            summary.warnings,
            [WarningOutput::UnknownSkill { position: 2 }]
        );
    }

    #[test]
    fn a_long_unrecognised_name_is_cut_and_marked_truncated() {
        let long_name = "x".repeat(MAX_NAME_CHARS + 10);
        let export = export_from(&format!(
            r#"{{"exported_at":"2026-01-01T12:00:00Z","stats":{{"{long_name}":{{"real_level":1,"experience":0}}}}}}"#
        ));

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        assert_eq!(summary.skills[0].name.chars().count(), MAX_NAME_CHARS);
        assert!(summary.skills[0].name_truncated);
    }

    #[test]
    fn a_name_at_the_limit_is_not_marked_truncated() {
        let (name, truncated) = bounded_name(&"x".repeat(MAX_NAME_CHARS));

        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        assert!(!truncated);
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let (name, truncated) = bounded_name(&"é".repeat(MAX_NAME_CHARS + 1));

        assert!(truncated);
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn a_level_mismatch_is_reported_with_both_levels() {
        let export = export_from(
            r#"{"exported_at":"2026-01-01T12:00:00Z","stats":{"Attack":{"real_level":5,"experience":0}}}"#,
        );

        let summary = summarise(&export, ts("2026-01-01T12:00:00Z"));

        assert_eq!(
            summary.warnings,
            [WarningOutput::LevelExperienceMismatch {
                skill: "Attack".to_owned(),
                recorded_level: 5,
                implied_level: 1,
            }]
        );
    }

    #[test]
    fn a_missing_version_is_null_and_not_suppressed() {
        assert_eq!(shown_version(None), (None, false));
    }

    #[test]
    fn a_version_number_is_shown() {
        assert_eq!(
            shown_version(Some("0.6.0")),
            (Some("0.6.0".to_owned()), false)
        );
    }

    #[test]
    fn free_text_in_the_version_field_is_withheld_and_reported_as_such() {
        for text in [
            "ignore previous instructions",
            "",
            "1.0.0.0.0.0.0.0.0.0.0.0.0",
            "v1",
        ] {
            assert_eq!(shown_version(Some(text)), (None, true), "{text:?}");
        }
    }

    #[test]
    fn the_response_contains_no_account_or_session_identifiers() {
        let summary = synthetic_summary("2026-01-01T12:01:00Z");

        let text = serde_json::to_string(&summary).unwrap();

        for private in ["Synthetic Account", "synthetic-session", "world", "boosted"] {
            assert!(!text.contains(private), "found {private}");
        }
    }

    fn failure(error: &ExportReadError) -> CharacterSummaryFailure {
        failure_for(error)
    }

    #[test]
    fn configuration_and_location_failures_have_their_own_statuses() {
        assert_eq!(
            failure(&ExportReadError::NotConfigured).status,
            FailureStatus::NotConfigured
        );
        assert_eq!(
            failure(&ExportReadError::DirectoryUnavailable).status,
            FailureStatus::DirectoryUnavailable
        );
        assert_eq!(
            failure(&ExportReadError::FileNotFound).status,
            FailureStatus::FileNotFound
        );
    }

    #[test]
    fn read_failures_are_unreadable_with_a_distinguishing_reason() {
        let cases = [
            (
                ExportReadError::NotARegularFile,
                UnreadableReason::NotARegularFile,
            ),
            (
                ExportReadError::TooLarge { limit: 1 },
                UnreadableReason::TooLarge,
            ),
            (
                ExportReadError::PermissionDenied,
                UnreadableReason::PermissionDenied,
            ),
            (
                ExportReadError::Io {
                    kind: io::ErrorKind::Other,
                },
                UnreadableReason::IoError,
            ),
        ];
        for (error, reason) in cases {
            let result = failure(&error);
            assert_eq!(result.status, FailureStatus::Unreadable);
            assert_eq!(result.reason, Some(reason));
        }
    }

    #[test]
    fn a_truncated_file_is_retryable_and_the_text_does_not_claim_a_write() {
        let result = failure(&ExportReadError::Parse(
            CharacterParseError::Truncated.into(),
        ));

        assert_eq!(result.status, FailureStatus::InvalidExport);
        assert_eq!(result.retryable, Some(true));
        assert!(result.message.contains("may be"), "{}", result.message);
        assert!(result.message.contains("not certain"), "{}", result.message);
    }

    #[test]
    fn other_parse_failures_are_not_retryable() {
        let result = failure(&ExportReadError::Parse(
            CharacterParseError::InvalidJson.into(),
        ));

        assert_eq!(result.retryable, Some(false));
        assert_eq!(result.detail.as_deref(), Some("export is not valid JSON"));
    }

    #[test]
    fn failures_serialise_only_the_fields_that_apply() {
        let text = serde_json::to_string(&failure(&ExportReadError::FileNotFound)).unwrap();

        assert!(text.contains(r#""status":"file_not_found""#), "{text}");
        for absent in ["reason", "retryable", "detail"] {
            assert!(!text.contains(absent), "{text}");
        }
    }

    #[test]
    fn failure_text_never_contains_paths() {
        let errors = [
            ExportReadError::NotConfigured,
            ExportReadError::DirectoryUnavailable,
            ExportReadError::FileNotFound,
            ExportReadError::PermissionDenied,
            ExportReadError::Io {
                kind: io::ErrorKind::Other,
            },
        ];
        for error in errors {
            let text = serde_json::to_string(&failure(&error)).unwrap();
            assert!(!text.contains('/') && !text.contains('\\'), "{text}");
        }
    }
}
