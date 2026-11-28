//! Parsing of Character State Exporter's diary observations.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

use super::ExportedAt;

/// Largest diary export accepted, in bytes.
pub const MAX_DIARY_EXPORT_BYTES: usize = 2 * 1024 * 1024;

const MAX_AREAS: usize = 64;
const MAX_TIERS_PER_AREA: usize = 16;
const MAX_TASKS_PER_TIER: usize = 128;
const MAX_LABEL_BYTES: usize = 128;
const MAX_TASK_NAME_BYTES: usize = 256;

/// Diary difficulty as recorded by the exporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiaryTier {
    /// Easy diary.
    Easy,
    /// Medium diary.
    Medium,
    /// Hard diary.
    Hard,
    /// Elite diary.
    Elite,
}

impl DiaryTier {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "easy" => Some(Self::Easy),
            "medium" => Some(Self::Medium),
            "hard" => Some(Self::Hard),
            "elite" => Some(Self::Elite),
            _ => None,
        }
    }
}

/// One named task observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiaryTask {
    /// Untrusted display text as exported.
    pub display_name: String,
    /// Exported task completion flag.
    pub complete: bool,
}

/// The independent observations supplied for a tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiaryTierProgress {
    /// Exported tier completion flag.
    pub complete: bool,
    /// Exported count of completed tasks.
    pub tasks_done: u16,
    /// Named tasks. An absent list is unrecorded.
    pub tasks: Option<Vec<DiaryTask>>,
}

/// Tiers observed for a single area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiaryArea {
    tiers: BTreeMap<DiaryTier, DiaryTierProgress>,
}

impl DiaryArea {
    /// Observed tiers. A missing tier is unrecorded.
    #[must_use]
    pub const fn tiers(&self) -> &BTreeMap<DiaryTier, DiaryTierProgress> {
        &self.tiers
    }
}

/// Diary observations from one export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiaryProgress {
    exported_at: ExportedAt,
    plugin_version: Option<String>,
    areas: BTreeMap<String, DiaryArea>,
    warnings: Vec<DiaryWarning>,
}

impl DiaryProgress {
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

    /// Observed areas by untrusted display name. A missing area is unrecorded.
    #[must_use]
    pub const fn areas(&self) -> &BTreeMap<String, DiaryArea> {
        &self.areas
    }

    /// Non-fatal findings in area name order, followed by any summary warning.
    #[must_use]
    pub fn warnings(&self) -> &[DiaryWarning] {
        &self.warnings
    }
}

/// A non-fatal finding. Area positions are one-based in lexicographic name order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiaryWarning {
    /// An unknown tier was omitted.
    UnknownTier {
        /// Position of the affected area.
        area: usize,
    },
    /// The count differs from the number of complete named tasks.
    TaskCountMismatch {
        /// Position of the affected area.
        area: usize,
        /// The affected tier.
        tier: DiaryTier,
    },
    /// A complete tier contains an incomplete named task.
    CompleteWithIncompleteTask {
        /// Position of the affected area.
        area: usize,
        /// The affected tier.
        tier: DiaryTier,
    },
    /// The supplied summary contradicts the observations.
    SummaryMismatch,
}

/// Why a diary export could not be parsed. Positions are one-based. Errors
/// report static field names and positions without raw input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DiaryParseError {
    /// The input exceeds the byte limit.
    #[error("diary export exceeds the byte limit")]
    InputTooLarge,
    /// The input ended prematurely.
    #[error("export ended unexpectedly (a write may be in progress)")]
    Truncated,
    /// The input is not valid JSON.
    #[error("export is not valid JSON")]
    InvalidJson,
    /// The top level is not an object.
    #[error("diary export must be an object")]
    UnexpectedShape,
    /// A required field is absent.
    #[error("required field {field} is missing")]
    MissingField {
        /// The field name.
        field: &'static str,
    },
    /// A top-level field or its contents are invalid.
    #[error("field {field} has an invalid value")]
    InvalidField {
        /// The field name.
        field: &'static str,
    },
    /// The area count exceeds the limit.
    #[error("diary export exceeds the area limit")]
    TooManyAreas,
    /// The area name exceeds the byte limit or the area is not an object.
    #[error("diary area {area} is invalid")]
    InvalidArea {
        /// Position of the affected area.
        area: usize,
    },
    /// The area has too many tier keys.
    #[error("diary area {area} exceeds the tier limit")]
    TooManyTiers {
        /// Position of the affected area.
        area: usize,
    },
    /// A tier name exceeds the byte limit or a known tier is not an object.
    #[error("diary tier in area {area} is invalid")]
    InvalidTier {
        /// Position of the affected area.
        area: usize,
    },
    /// A tier field is missing or invalid.
    #[error("diary area {area} tier {tier:?} field {field} is missing or invalid")]
    InvalidTierField {
        /// Position of the affected area.
        area: usize,
        /// The affected tier.
        tier: DiaryTier,
        /// The field name.
        field: &'static str,
    },
    /// A tier has too many tasks.
    #[error("diary area {area} tier {tier:?} exceeds the task limit")]
    TooManyTasks {
        /// Position of the affected area.
        area: usize,
        /// The affected tier.
        tier: DiaryTier,
    },
    /// A task is not an object.
    #[error("diary area {area} tier {tier:?} task {task} must be an object")]
    InvalidTask {
        /// Position of the affected area.
        area: usize,
        /// The affected tier.
        tier: DiaryTier,
        /// Position of the affected task.
        task: usize,
    },
    /// A task field is missing or invalid.
    #[error("diary area {area} tier {tier:?} task {task} field {field} is missing or invalid")]
    InvalidTaskField {
        /// Position of the affected area.
        area: usize,
        /// The affected tier.
        tier: DiaryTier,
        /// Position of the affected task.
        task: usize,
        /// The field name.
        field: &'static str,
    },
}

impl DiaryParseError {
    /// Whether retrying shortly may succeed if the export was being written.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

type Fields = Map<String, Value>;

/// Parses a diary export without I/O.
///
/// Limits are 64 areas, 16 tier keys per area
/// and 128 tasks per tier. Area and tier keys, timestamp and plugin version
/// may use at most 128 UTF-8 bytes, and task names at most 256 UTF-8 bytes.
///
/// # Errors
///
/// Returns [`DiaryParseError`] on unsupported structure, malformed input or a
/// limit violation. Disagreement between observations is recorded as warnings.
pub fn parse_diary_export(bytes: &[u8]) -> Result<DiaryProgress, DiaryParseError> {
    if bytes.len() > MAX_DIARY_EXPORT_BYTES {
        return Err(DiaryParseError::InputTooLarge);
    }
    let input: Value = serde_json::from_slice(bytes).map_err(|error| {
        if error.is_eof() {
            DiaryParseError::Truncated
        } else {
            DiaryParseError::InvalidJson
        }
    })?;
    let fields = input.as_object().ok_or(DiaryParseError::UnexpectedShape)?;
    let (exported_at, plugin_version) = read_envelope(fields)?;
    let diaries = required(fields, "diaries")?
        .as_object()
        .ok_or(DiaryParseError::InvalidField { field: "diaries" })?;
    let mut warnings = Vec::new();
    let areas = read_areas(diaries, &mut warnings)?;
    if let Some(summary) = fields.get("summary") {
        check_summary(summary, &areas, &mut warnings)?;
    }
    Ok(DiaryProgress {
        exported_at,
        plugin_version,
        areas,
        warnings,
    })
}

fn required<'a>(fields: &'a Fields, field: &'static str) -> Result<&'a Value, DiaryParseError> {
    fields
        .get(field)
        .ok_or(DiaryParseError::MissingField { field })
}

fn read_envelope(fields: &Fields) -> Result<(ExportedAt, Option<String>), DiaryParseError> {
    let exported_at = required(fields, "exported_at")?
        .as_str()
        .filter(|text| text.len() <= MAX_LABEL_BYTES)
        .and_then(ExportedAt::parse)
        .ok_or(DiaryParseError::InvalidField {
            field: "exported_at",
        })?;
    let plugin_version = match fields.get("plugin_version") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.len() <= MAX_LABEL_BYTES => Some(text.clone()),
        Some(_) => {
            return Err(DiaryParseError::InvalidField {
                field: "plugin_version",
            });
        }
    };
    Ok((exported_at, plugin_version))
}

fn read_areas(
    diaries: &Fields,
    warnings: &mut Vec<DiaryWarning>,
) -> Result<BTreeMap<String, DiaryArea>, DiaryParseError> {
    if diaries.len() > MAX_AREAS {
        return Err(DiaryParseError::TooManyAreas);
    }
    diaries
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            let area = index + 1;
            let fields = value
                .as_object()
                .filter(|_| name.len() <= MAX_LABEL_BYTES)
                .ok_or(DiaryParseError::InvalidArea { area })?;
            let tiers = read_tiers(fields, area, warnings)?;
            Ok((name.clone(), DiaryArea { tiers }))
        })
        .collect()
}

fn read_tiers(
    fields: &Fields,
    area: usize,
    warnings: &mut Vec<DiaryWarning>,
) -> Result<BTreeMap<DiaryTier, DiaryTierProgress>, DiaryParseError> {
    if fields.len() > MAX_TIERS_PER_AREA {
        return Err(DiaryParseError::TooManyTiers { area });
    }
    let mut tiers = BTreeMap::new();
    for (key, value) in fields {
        if key.len() > MAX_LABEL_BYTES {
            return Err(DiaryParseError::InvalidTier { area });
        }
        let Some(tier) = DiaryTier::from_key(key) else {
            warnings.push(DiaryWarning::UnknownTier { area });
            continue;
        };
        let observation = read_tier(value, area, tier)?;
        assess_tier(&observation, area, tier, warnings);
        tiers.insert(tier, observation);
    }
    Ok(tiers)
}

fn read_tier(
    value: &Value,
    area: usize,
    tier: DiaryTier,
) -> Result<DiaryTierProgress, DiaryParseError> {
    let fields = value
        .as_object()
        .ok_or(DiaryParseError::InvalidTier { area })?;
    let invalid = |field| DiaryParseError::InvalidTierField { area, tier, field };
    let complete = fields
        .get("complete")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("complete"))?;
    let tasks_done = fields
        .get("tasks_done")
        .and_then(Value::as_u64)
        .and_then(|count| u16::try_from(count).ok())
        .ok_or_else(|| invalid("tasks_done"))?;
    let tasks = fields
        .get("tasks")
        .map(|value| read_tasks(value, area, tier))
        .transpose()?;
    Ok(DiaryTierProgress {
        complete,
        tasks_done,
        tasks,
    })
}

fn read_tasks(
    value: &Value,
    area: usize,
    tier: DiaryTier,
) -> Result<Vec<DiaryTask>, DiaryParseError> {
    let tasks = value.as_array().ok_or(DiaryParseError::InvalidTierField {
        area,
        tier,
        field: "tasks",
    })?;
    if tasks.len() > MAX_TASKS_PER_TIER {
        return Err(DiaryParseError::TooManyTasks { area, tier });
    }
    tasks
        .iter()
        .enumerate()
        .map(|(index, value)| read_task(value, area, tier, index + 1))
        .collect()
}

fn read_task(
    value: &Value,
    area: usize,
    tier: DiaryTier,
    task: usize,
) -> Result<DiaryTask, DiaryParseError> {
    let fields = value
        .as_object()
        .ok_or(DiaryParseError::InvalidTask { area, tier, task })?;
    let invalid = |field| DiaryParseError::InvalidTaskField {
        area,
        tier,
        task,
        field,
    };
    let display_name = fields
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| name.len() <= MAX_TASK_NAME_BYTES)
        .ok_or_else(|| invalid("name"))?
        .to_owned();
    let complete = fields
        .get("complete")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("complete"))?;
    Ok(DiaryTask {
        display_name,
        complete,
    })
}

fn assess_tier(
    observation: &DiaryTierProgress,
    area: usize,
    tier: DiaryTier,
    warnings: &mut Vec<DiaryWarning>,
) {
    if let Some(tasks) = &observation.tasks {
        let completed = tasks.iter().filter(|task| task.complete).count();
        if completed != usize::from(observation.tasks_done) {
            warnings.push(DiaryWarning::TaskCountMismatch { area, tier });
        }
        if observation.complete && completed < tasks.len() {
            warnings.push(DiaryWarning::CompleteWithIncompleteTask { area, tier });
        }
    }
}

fn check_summary(
    summary: &Value,
    areas: &BTreeMap<String, DiaryArea>,
    warnings: &mut Vec<DiaryWarning>,
) -> Result<(), DiaryParseError> {
    let invalid = DiaryParseError::InvalidField { field: "summary" };
    let summary = summary.as_object().ok_or(invalid)?;
    let count = |field| summary.get(field).and_then(Value::as_u64).ok_or(invalid);
    let total = count("tiers_possible")?;
    let complete = count("tiers_complete")?;

    let mut observed_complete = 0;
    let mut observed_incomplete = 0;
    for tier in areas.values().flat_map(|area| area.tiers.values()) {
        if tier.complete {
            observed_complete += 1;
        } else {
            observed_incomplete += 1;
        }
    }
    // Unrecorded or unknown tiers could account for any excess in either category.
    if complete < observed_complete
        || total
            .checked_sub(complete)
            .is_none_or(|incomplete| incomplete < observed_incomplete)
    {
        warnings.push(DiaryWarning::SummaryMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const DIARIES: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/diaries.synthetic.json");

    fn export() -> Value {
        serde_json::from_slice(DIARIES).unwrap()
    }

    fn parse(value: &Value) -> Result<DiaryProgress, DiaryParseError> {
        parse_diary_export(value.to_string().as_bytes())
    }

    #[test]
    fn conflicting_observations_are_preserved() {
        let parsed = parse_diary_export(DIARIES).unwrap();
        assert_eq!(
            parsed.exported_at().text(),
            "2026-01-02T08:20:30.123456789+01:00"
        );
        assert_eq!(parsed.plugin_version(), Some("0.6.0-synthetic"));
        let area = &parsed.areas()["Example Region"];
        let easy = &area.tiers()[&DiaryTier::Easy];
        assert!(easy.complete);
        assert_eq!(easy.tasks_done, 1);
        let task = &easy.tasks.as_ref().unwrap()[0];
        assert_eq!(task.display_name, "Example task");
        assert!(!task.complete);
        assert_eq!(area.tiers()[&DiaryTier::Medium].tasks, None);
        assert_eq!(area.tiers().get(&DiaryTier::Hard), None);
        assert_eq!(
            parsed.warnings(),
            &[
                DiaryWarning::TaskCountMismatch {
                    area: 1,
                    tier: DiaryTier::Easy
                },
                DiaryWarning::CompleteWithIncompleteTask {
                    area: 1,
                    tier: DiaryTier::Easy
                },
                DiaryWarning::SummaryMismatch,
            ]
        );
    }

    #[test]
    fn missing_and_explicitly_empty_tasks_are_distinct() {
        let mut input = export();
        input["diaries"]["Example Region"]["medium"]["tasks"] = json!([]);
        let parsed = parse(&input).unwrap();
        assert_eq!(
            parsed.areas()["Example Region"].tiers()[&DiaryTier::Medium].tasks,
            Some(Vec::new())
        );
        input["diaries"] = json!({});
        let parsed = parse(&input).unwrap();
        assert!(parsed.areas().is_empty());
        assert_eq!(parsed.warnings(), []);
        input.as_object_mut().unwrap().remove("diaries");
        assert_eq!(
            parse(&input),
            Err(DiaryParseError::MissingField { field: "diaries" })
        );
    }

    #[test]
    fn new_tiers_warn_without_exposing_or_interpreting_the_key() {
        let mut input = export();
        input["diaries"] = json!({"Example Region": {"UNTRUSTED-TIER": true}});
        let parsed = parse(&input).unwrap();
        assert!(parsed.areas()["Example Region"].tiers().is_empty());
        assert_eq!(parsed.warnings(), &[DiaryWarning::UnknownTier { area: 1 }]);
        assert!(!format!("{:?}", parsed.warnings()).contains("UNTRUSTED-TIER"));
    }

    #[test]
    fn summary_checks_use_only_observable_contradictions() {
        let mut input = export();
        input["diaries"]["Example Region"]["easy"]
            .as_object_mut()
            .unwrap()
            .remove("tasks");
        input["summary"] = json!({"tiers_complete": 2, "tiers_possible": 6});
        assert_eq!(parse(&input).unwrap().warnings(), []);

        input["summary"] = json!({"tiers_complete": 3, "tiers_possible": 3});
        assert_eq!(
            parse(&input).unwrap().warnings(),
            &[DiaryWarning::SummaryMismatch]
        );

        input["summary"] = json!({"tiers_complete": 2, "tiers_possible": 2});
        assert_eq!(
            parse(&input).unwrap().warnings(),
            &[DiaryWarning::SummaryMismatch]
        );
    }

    #[test]
    fn rejects_excessive_input_before_json_and_distinguishes_truncation() {
        assert_eq!(
            parse_diary_export(&vec![b'!'; MAX_DIARY_EXPORT_BYTES + 1]),
            Err(DiaryParseError::InputTooLarge)
        );
        assert_eq!(parse_diary_export(b"{"), Err(DiaryParseError::Truncated));
        assert!(DiaryParseError::Truncated.is_transient());
        assert!(!DiaryParseError::InvalidJson.is_transient());
        assert_eq!(parse_diary_export(b"{]"), Err(DiaryParseError::InvalidJson));
        assert_eq!(
            parse_diary_export(b"[]"),
            Err(DiaryParseError::UnexpectedShape)
        );
    }

    #[test]
    fn area_tier_and_task_counts_are_bounded() {
        let mut input = export();
        input["diaries"] = Value::Object(
            (0..65)
                .map(|index| (format!("Region {index}"), json!({})))
                .collect(),
        );
        assert_eq!(parse(&input), Err(DiaryParseError::TooManyAreas));

        input["diaries"] = json!({"Example Region": Value::Object((0..17).map(|index| (format!("Tier {index}"), json!({}))).collect())});
        assert_eq!(
            parse(&input),
            Err(DiaryParseError::TooManyTiers { area: 1 })
        );

        let task = json!({"name": "Example task", "complete": false});
        input["diaries"] = json!({"Example Region": {"easy": {"complete": false, "tasks_done": 0, "tasks": vec![task; 129]}}});
        assert_eq!(
            parse(&input),
            Err(DiaryParseError::TooManyTasks {
                area: 1,
                tier: DiaryTier::Easy
            })
        );
    }

    #[test]
    fn names_are_bounded_by_decoded_utf8_bytes() {
        let mut input = export();
        let name = "é".repeat(128);
        input["diaries"]["Example Region"]["easy"]["tasks"][0]["name"] = json!(name);
        assert!(parse(&input).is_ok());

        input["diaries"]["Example Region"]["easy"]["tasks"][0]["name"] =
            json!(format!("{name}MARKER"));
        let error = parse(&input).unwrap_err();
        assert_eq!(
            error,
            DiaryParseError::InvalidTaskField {
                area: 1,
                tier: DiaryTier::Easy,
                task: 1,
                field: "name"
            }
        );
        assert!(!format!("{error:?} {error}").contains("MARKER"));
    }

    #[test]
    fn invalid_types_and_bounds_are_typed_without_input_values() {
        let mut input = export();
        input["diaries"]["Example Region"]["easy"]["tasks_done"] = json!(65_536);
        assert_eq!(
            parse(&input),
            Err(DiaryParseError::InvalidTierField {
                area: 1,
                tier: DiaryTier::Easy,
                field: "tasks_done"
            })
        );
        input["diaries"]["Example Region"]["easy"]["tasks_done"] = json!(0);
        input["diaries"]["Example Region"]["easy"]["tasks"] = Value::Null;
        assert_eq!(
            parse(&input),
            Err(DiaryParseError::InvalidTierField {
                area: 1,
                tier: DiaryTier::Easy,
                field: "tasks"
            })
        );
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
                Some(DiaryParseError::InvalidField {
                    field: "plugin_version"
                })
            );
        }
        input["plugin_version"] = Value::Null;
        assert_eq!(plugin_version(&input), Ok(None));
        input.as_object_mut().unwrap().remove("plugin_version");
        assert_eq!(plugin_version(&input), Ok(None));

        input["exported_at"] = json!("yesterday");
        assert_eq!(
            error(&input),
            Some(DiaryParseError::InvalidField {
                field: "exported_at"
            })
        );
        input.as_object_mut().unwrap().remove("exported_at");
        assert_eq!(
            error(&input),
            Some(DiaryParseError::MissingField {
                field: "exported_at"
            })
        );
    }
}
