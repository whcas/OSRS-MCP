//! Parsing of the Character State Exporter's `character.json`.
//!
//! Decisions are in `docs/research/runelite-export-formats.md`. In short:
//! unknown fields are ignored, `boosted_level`, `session_id` and
//! `account_name` are never read or retained, a level that disagrees with
//! experience is a warning, and error values never contain input values apart
//! from a skill name cut to [`MAX_NAME_CHARS`] characters.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::ops::RangeInclusive;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;
use thiserror::Error;

use super::experience::{MAX_EXPERIENCE, MAX_LEVEL, level_for_experience};
use super::skill::Skill;
use super::timestamp::ExportedAt;

/// A level and experience for one skill, as exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRecord {
    /// Which skill this is.
    pub skill: Skill,
    /// The real (unboosted) level, `1..=99`.
    pub real_level: u8,
    /// Total experience, `0..=200_000_000`.
    pub experience: u32,
}

/// Something worth reporting that did not stop the import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// The recorded level differs from the level the experience implies.
    LevelExperienceMismatch {
        /// The skill concerned.
        skill: Skill,
        /// The level in the export.
        recorded_level: u8,
        /// The level implied by the experience table.
        implied_level: u8,
    },
    /// A skill name this importer does not know. It is kept, unchecked.
    UnknownSkill {
        /// The name as exported.
        name: String,
    },
}

/// A successfully imported `character.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterExport {
    exported_at: ExportedAt,
    plugin_version: Option<String>,
    skills: Vec<SkillRecord>,
    warnings: Vec<Warning>,
}

impl CharacterExport {
    /// When the plugin wrote the file. This is the time of the last *change*,
    /// not the last observation: unchanged data is not rewritten.
    #[must_use]
    pub const fn exported_at(&self) -> &ExportedAt {
        &self.exported_at
    }

    /// The plugin version recorded in the file. Any JSON string is accepted
    /// as is; a missing field or an explicit `null` is `None`.
    #[must_use]
    pub fn plugin_version(&self) -> Option<&str> {
        self.plugin_version.as_deref()
    }

    /// Every skill, in the order the file listed them.
    #[must_use]
    pub fn skills(&self) -> &[SkillRecord] {
        &self.skills
    }

    /// One skill, if the file contained it.
    #[must_use]
    pub fn skill(&self, skill: &Skill) -> Option<&SkillRecord> {
        self.skills.iter().find(|record| &record.skill == skill)
    }

    /// Non-fatal findings, in file order.
    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }
}

/// Why a skill value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillValueProblem {
    /// Not a non-negative JSON integer (a string, a fraction, a negative
    /// number, `null`, or too large to represent).
    NotAnInteger,
    /// An integer outside the allowed range.
    OutOfRange,
}

impl fmt::Display for SkillValueProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAnInteger => "is not a non-negative integer",
            Self::OutOfRange => "is out of range",
        })
    }
}

/// Why `character.json` could not be imported. No variant carries an input
/// value other than a skill name, which is cut to [`MAX_NAME_CHARS`]
/// characters so that a hostile file cannot produce an unbounded diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CharacterParseError {
    /// The input ended inside a JSON value, or was empty. The plugin does not
    /// write atomically, so this usually means a write was in progress: retry.
    #[error("export ended unexpectedly (a write may be in progress)")]
    Truncated,
    /// The input is not valid JSON.
    #[error("export is not valid JSON")]
    InvalidJson,
    /// Valid JSON of an unexpected shape, for example not an object.
    #[error("export JSON has an unexpected shape")]
    UnexpectedShape,
    /// A required top-level field is absent.
    #[error("required field `{field}` is missing")]
    MissingField {
        /// The field name.
        field: &'static str,
    },
    /// `exported_at` is not a timestamp the plugin writes.
    #[error("`exported_at` is not a recognised timestamp")]
    InvalidTimestamp,
    /// `plugin_version` is present but not a string.
    #[error("`plugin_version` is not a string")]
    InvalidPluginVersion,
    /// `stats` is an empty object.
    #[error("`stats` contains no skills")]
    NoSkills,
    /// A skill appears twice in `stats`.
    #[error("skill `{name}` appears more than once")]
    DuplicateSkill {
        /// The skill name.
        name: String,
    },
    /// A skill lacks a required value.
    #[error("skill `{skill}` is missing `{field}`")]
    MissingSkillField {
        /// The skill name.
        skill: String,
        /// The missing field.
        field: &'static str,
    },
    /// A skill value is not acceptable.
    #[error("skill `{skill}` field `{field}` {problem}")]
    InvalidSkillValue {
        /// The skill name.
        skill: String,
        /// The field with the bad value.
        field: &'static str,
        /// What is wrong with it.
        problem: SkillValueProblem,
    },
}

impl CharacterParseError {
    /// Whether retrying shortly may succeed, because the file was probably
    /// caught mid-write.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::Truncated)
    }
}

/// Classifies a JSON failure by kind only. The serde error is not kept: its
/// text can quote the input, which may be private.
impl From<serde_json::Error> for CharacterParseError {
    fn from(error: serde_json::Error) -> Self {
        match error.classify() {
            serde_json::error::Category::Eof => Self::Truncated,
            serde_json::error::Category::Syntax | serde_json::error::Category::Io => {
                Self::InvalidJson
            }
            serde_json::error::Category::Data => Self::UnexpectedShape,
        }
    }
}

/// Parses the bytes of one `character.json`.
///
/// # Errors
///
/// Returns [`CharacterParseError`] when the input is not valid JSON, lacks a
/// required field, or holds a value outside the documented ranges. A level that
/// disagrees with experience is reported through
/// [`CharacterExport::warnings`], not as an error.
pub fn parse_character_export(bytes: &[u8]) -> Result<CharacterExport, CharacterParseError> {
    // Fields stay as raw JSON text so that `stats` can be read in file order,
    // which a `Value` map would not allow. Unknown fields are never examined.
    let fields: Fields = serde_json::from_slice(bytes)?;

    let exported_at = read_exported_at(&fields)?;
    let plugin_version = read_plugin_version(&fields)?;
    let skills = read_skills(&fields)?;
    // Warnings are advice about records that already parsed; they never decide
    // whether the import succeeds.
    let warnings = skills.iter().filter_map(warning_for).collect();

    Ok(CharacterExport {
        exported_at,
        plugin_version,
        skills,
        warnings,
    })
}

/// The top-level fields of the document, each still as raw JSON text.
type Fields = BTreeMap<String, Box<RawValue>>;

/// A top-level field as a value, or `None` if the file has no such field. An
/// explicit JSON `null` is `Some(Value::Null)`, not missing.
fn read_value(fields: &Fields, name: &str) -> Result<Option<Value>, CharacterParseError> {
    fields
        .get(name)
        .map(|raw| serde_json::from_str(raw.get()))
        .transpose()
        .map_err(CharacterParseError::from)
}

/// `exported_at`: required, and a timestamp string the plugin could write.
fn read_exported_at(fields: &Fields) -> Result<ExportedAt, CharacterParseError> {
    match read_value(fields, "exported_at")? {
        None => Err(CharacterParseError::MissingField {
            field: "exported_at",
        }),
        Some(Value::String(text)) => {
            ExportedAt::parse(&text).ok_or(CharacterParseError::InvalidTimestamp)
        }
        Some(_) => Err(CharacterParseError::InvalidTimestamp),
    }
}

/// `plugin_version`: any string is kept as text; a missing field or an
/// explicit `null` is `None`; any other JSON type is an error.
fn read_plugin_version(fields: &Fields) -> Result<Option<String>, CharacterParseError> {
    match read_value(fields, "plugin_version")? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(version)) => Ok(Some(version)),
        Some(_) => Err(CharacterParseError::InvalidPluginVersion),
    }
}

/// The skills in `stats`, in file order. The object must be present, non-empty
/// and free of repeated skill names. Each entry is validated as it is read, so
/// the first problem in file order is the one reported.
fn read_skills(fields: &Fields) -> Result<Vec<SkillRecord>, CharacterParseError> {
    let stats = fields
        .get("stats")
        .ok_or(CharacterParseError::MissingField { field: "stats" })?;
    let SkillEntries(entries) = serde_json::from_str(stats.get())?;
    if entries.is_empty() {
        return Err(CharacterParseError::NoSkills);
    }

    let mut seen = HashSet::new();
    let mut skills = Vec::with_capacity(entries.len());
    for (name, value) in &entries {
        if !seen.insert(name.as_str()) {
            return Err(CharacterParseError::DuplicateSkill {
                name: diagnostic_name(name),
            });
        }
        skills.push(read_skill(name, value)?);
    }
    Ok(skills)
}

fn read_skill(name: &str, value: &Value) -> Result<SkillRecord, CharacterParseError> {
    let object = value
        .as_object()
        .ok_or(CharacterParseError::UnexpectedShape)?;
    let real_level = read_integer(name, "real_level", object.get("real_level"), 1..=MAX_LEVEL)?;
    let experience = read_integer(
        name,
        "experience",
        object.get("experience"),
        0..=MAX_EXPERIENCE,
    )?;
    Ok(SkillRecord {
        skill: Skill::from_name(name),
        real_level,
        experience,
    })
}

/// The longest skill name, in characters, that an error will carry.
const MAX_NAME_CHARS: usize = 64;

/// A skill name as it may appear in an error: bounded, because the name comes
/// from the file.
fn diagnostic_name(name: &str) -> String {
    name.chars().take(MAX_NAME_CHARS).collect()
}

/// The integer in `value`, which must be present and within `range`.
fn read_integer<T>(
    skill: &str,
    field: &'static str,
    value: Option<&Value>,
    range: RangeInclusive<T>,
) -> Result<T, CharacterParseError>
where
    T: TryFrom<u64> + PartialOrd,
{
    let invalid = |problem| CharacterParseError::InvalidSkillValue {
        skill: diagnostic_name(skill),
        field,
        problem,
    };
    let value = value.ok_or_else(|| CharacterParseError::MissingSkillField {
        skill: diagnostic_name(skill),
        field,
    })?;
    let number = value
        .as_u64()
        .ok_or_else(|| invalid(SkillValueProblem::NotAnInteger))?;
    T::try_from(number)
        .ok()
        .filter(|candidate| range.contains(candidate))
        .ok_or_else(|| invalid(SkillValueProblem::OutOfRange))
}

/// The warning for one record, if any. Unknown skills are not checked against
/// the experience table because it may not apply to them.
fn warning_for(record: &SkillRecord) -> Option<Warning> {
    if !record.skill.is_known() {
        return Some(Warning::UnknownSkill {
            name: record.skill.name().to_owned(),
        });
    }
    let implied_level = level_for_experience(record.experience);
    (implied_level != record.real_level).then(|| Warning::LevelExperienceMismatch {
        skill: record.skill.clone(),
        recorded_level: record.real_level,
        implied_level,
    })
}

/// The entries of the `stats` object in file order, so that a repeated skill
/// name is detectable (a map would keep only the last one). Anything that is
/// not a JSON object fails to deserialise.
struct SkillEntries(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for SkillEntries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;

        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = SkillEntries;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of skills")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(SkillEntries(entries))
            }
        }

        deserializer.deserialize_map(EntriesVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid document with one `Attack` skill, with `stats_body`
    /// spliced in so each test states only what it varies.
    fn document(exported_at: &str, stats_body: &str) -> String {
        format!(r#"{{"exported_at":"{exported_at}","stats":{{{stats_body}}}}}"#)
    }

    fn attack(real_level: &str, experience: &str) -> String {
        format!(r#""Attack":{{"real_level":{real_level},"experience":{experience}}}"#)
    }

    fn parse(text: &str) -> Result<CharacterExport, CharacterParseError> {
        parse_character_export(text.as_bytes())
    }

    #[test]
    fn unknown_fields_are_ignored_at_every_level() {
        let text = r#"{"exported_at":"2026-01-01T00:00Z","future":{"a":1},
            "stats":{"Attack":{"real_level":1,"experience":0,"boosted_level":1,"extra":[1,2]}}}"#;
        let export = parse(text).expect("unknown fields are ignored");
        assert_eq!(export.skills().len(), 1);
    }

    #[test]
    fn a_bad_timestamp_in_a_document_is_a_typed_error() {
        let result = parse(&document("not a time", &attack("1", "0")));
        assert_eq!(result.unwrap_err(), CharacterParseError::InvalidTimestamp);
    }

    #[test]
    fn missing_required_fields_are_typed_errors() {
        assert_eq!(
            parse(r#"{"stats":{"Attack":{"real_level":1,"experience":0}}}"#).unwrap_err(),
            CharacterParseError::MissingField {
                field: "exported_at"
            }
        );
        assert_eq!(
            parse(r#"{"exported_at":"2026-01-01T00:00Z"}"#).unwrap_err(),
            CharacterParseError::MissingField { field: "stats" }
        );
    }

    #[test]
    fn empty_stats_are_rejected() {
        assert_eq!(
            parse(&document("2026-01-01T00:00Z", "")).unwrap_err(),
            CharacterParseError::NoSkills
        );
    }

    #[test]
    fn a_skill_missing_a_required_value_names_the_skill_and_field() {
        let text = document("2026-01-01T00:00Z", r#""Attack":{"experience":0}"#);
        assert_eq!(
            parse(&text).unwrap_err(),
            CharacterParseError::MissingSkillField {
                skill: "Attack".to_owned(),
                field: "real_level"
            }
        );
        let text = document("2026-01-01T00:00Z", r#""Attack":{"real_level":1}"#);
        assert_eq!(
            parse(&text).unwrap_err(),
            CharacterParseError::MissingSkillField {
                skill: "Attack".to_owned(),
                field: "experience"
            }
        );
    }

    #[test]
    fn values_that_are_not_integers_are_rejected() {
        for bad in ["1.5", "\"1\"", "null", "true", "-1", "1e2"] {
            let err = parse(&document("2026-01-01T00:00Z", &attack(bad, "0"))).unwrap_err();
            assert!(
                matches!(
                    err,
                    CharacterParseError::InvalidSkillValue {
                        field: "real_level",
                        problem: SkillValueProblem::NotAnInteger,
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
        }
    }

    #[test]
    fn level_and_experience_boundaries_are_enforced() {
        let ok = |level: &str, xp: &str| parse(&document("2026-01-01T00:00Z", &attack(level, xp)));
        assert!(ok("1", "0").is_ok());
        assert!(ok("99", "200000000").is_ok());

        assert!(matches!(
            ok("0", "0").unwrap_err(),
            CharacterParseError::InvalidSkillValue {
                field: "real_level",
                problem: SkillValueProblem::OutOfRange,
                ..
            }
        ));
        assert!(matches!(
            ok("100", "0").unwrap_err(),
            CharacterParseError::InvalidSkillValue {
                field: "real_level",
                problem: SkillValueProblem::OutOfRange,
                ..
            }
        ));
        assert!(matches!(
            ok("99", "200000001").unwrap_err(),
            CharacterParseError::InvalidSkillValue {
                field: "experience",
                problem: SkillValueProblem::OutOfRange,
                ..
            }
        ));
        // Larger than any integer type the importer uses: still a range error.
        assert!(matches!(
            ok("99", "99999999999999999999999").unwrap_err(),
            CharacterParseError::InvalidSkillValue {
                field: "experience",
                ..
            }
        ));
    }

    #[test]
    fn a_level_that_disagrees_with_experience_is_a_warning_not_an_error() {
        let text = document("2026-01-01T00:00Z", &attack("50", "83"));
        let export = parse(&text).expect("inconsistency is not fatal");
        assert_eq!(export.skill(&Skill::Attack).map(|s| s.real_level), Some(50));
        assert_eq!(
            export.warnings(),
            [Warning::LevelExperienceMismatch {
                skill: Skill::Attack,
                recorded_level: 50,
                implied_level: 2
            }]
        );
    }

    #[test]
    fn experience_boundaries_imply_the_right_level_without_warnings() {
        for (level, xp) in [
            (1, 0),
            (1, 82),
            (2, 83),
            (97, 11_805_605),
            (98, 11_805_606),
            (98, 13_034_430),
            (99, 13_034_431),
            (99, 200_000_000),
        ] {
            let text = document(
                "2026-01-01T00:00Z",
                &attack(&level.to_string(), &xp.to_string()),
            );
            let export = parse(&text).expect("valid");
            assert!(
                export.warnings().is_empty(),
                "level {level} xp {xp}: {:?}",
                export.warnings()
            );
        }
    }

    #[test]
    fn unknown_skills_are_kept_with_a_warning_and_not_checked_against_the_table() {
        let text = document(
            "2026-01-01T00:00Z",
            r#""Necromancy":{"real_level":50,"experience":83}"#,
        );
        let export = parse(&text).expect("unknown skills do not fail the import");
        let name = Skill::Unknown("Necromancy".to_owned());
        assert_eq!(export.skill(&name).map(|s| s.real_level), Some(50));
        assert_eq!(
            export.warnings(),
            [Warning::UnknownSkill {
                name: "Necromancy".to_owned()
            }]
        );
    }

    #[test]
    fn skills_are_returned_in_the_order_the_file_lists_them() {
        let body = format!(
            "{},{},{}",
            r#""Mining":{"real_level":1,"experience":0}"#,
            r#""Attack":{"real_level":1,"experience":0}"#,
            r#""Sailing":{"real_level":1,"experience":0}"#,
        );
        let export = parse(&document("2026-01-01T00:00Z", &body)).expect("valid");
        let order: Vec<_> = export
            .skills()
            .iter()
            .map(|record| record.skill.name())
            .collect();
        assert_eq!(order, ["Mining", "Attack", "Sailing"]);
    }

    #[test]
    fn a_skill_listed_twice_is_an_error() {
        let body = format!("{},{}", attack("1", "0"), attack("2", "83"));
        assert_eq!(
            parse(&document("2026-01-01T00:00Z", &body)).unwrap_err(),
            CharacterParseError::DuplicateSkill {
                name: "Attack".to_owned()
            }
        );
    }

    #[test]
    fn input_that_is_not_an_object_is_an_unexpected_shape() {
        // A derived `Deserialize` would accept the array forms by position.
        for text in [
            "[]",
            r#"["2026-01-01T00:00Z",null,{"Attack":{"real_level":1,"experience":0}}]"#,
            "42",
            "null",
            r#""text""#,
            r#"{"exported_at":"2026-01-01T00:00Z","stats":[]}"#,
            r#"{"exported_at":"2026-01-01T00:00Z","stats":null}"#,
            r#"{"exported_at":"2026-01-01T00:00Z","stats":{"Attack":[1,0]}}"#,
            r#"{"exported_at":"2026-01-01T00:00Z","stats":{"Attack":5}}"#,
        ] {
            assert_eq!(
                parse(text).unwrap_err(),
                CharacterParseError::UnexpectedShape,
                "{text}"
            );
        }
    }

    #[test]
    fn a_non_string_timestamp_is_an_invalid_timestamp_and_bad_bytes_do_not_panic() {
        assert_eq!(
            parse(r#"{"exported_at":5,"stats":{}}"#).unwrap_err(),
            CharacterParseError::InvalidTimestamp
        );
        assert!(parse_character_export(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn any_plugin_version_string_is_recorded_and_missing_or_null_is_none() {
        let with_version = |version: &str| {
            format!(
                r#"{{"exported_at":"2026-01-01T00:00Z","plugin_version":{version},"stats":{{{}}}}}"#,
                attack("1", "0")
            )
        };
        let version = |text: &str| {
            parse(&with_version(text)).map(|export| export.plugin_version().map(str::to_owned))
        };
        assert_eq!(
            version(r#""9.9.9-future""#),
            Ok(Some("9.9.9-future".to_owned()))
        );
        assert_eq!(version(r#""""#), Ok(Some(String::new())));
        assert_eq!(version("null"), Ok(None));
        let without = format!(
            r#"{{"exported_at":"2026-01-01T00:00Z","stats":{{{}}}}}"#,
            attack("1", "0")
        );
        assert_eq!(parse(&without).expect("valid").plugin_version(), None);
    }

    #[test]
    fn a_plugin_version_that_is_not_a_string_is_rejected() {
        for bad in ["6", "1.5", "true", "[]", r#"{"v":1}"#] {
            let text = format!(
                r#"{{"exported_at":"2026-01-01T00:00Z","plugin_version":{bad},"stats":{{{}}}}}"#,
                attack("1", "0")
            );
            assert_eq!(
                parse(&text).unwrap_err(),
                CharacterParseError::InvalidPluginVersion,
                "{bad}"
            );
        }
    }

    #[test]
    fn unknown_skills_are_still_held_to_the_level_and_experience_bounds() {
        for (level, experience) in [("0", "0"), ("100", "0"), ("50", "200000001")] {
            let body =
                format!(r#""Necromancy":{{"real_level":{level},"experience":{experience}}}"#);
            assert!(
                matches!(
                    parse(&document("2026-01-01T00:00Z", &body)).unwrap_err(),
                    CharacterParseError::InvalidSkillValue {
                        problem: SkillValueProblem::OutOfRange,
                        ..
                    }
                ),
                "{level} {experience}"
            );
        }
    }

    #[test]
    fn skill_names_in_errors_are_bounded() {
        let long = "x".repeat(1_000_000);
        let missing = parse(&document("2026-01-01T00:00Z", &format!(r#""{long}":{{}}"#)));
        let repeated = format!(
            r#""{long}":{{"real_level":1,"experience":0}},"{long}":{{"real_level":1,"experience":0}}"#
        );
        let duplicate = parse(&document("2026-01-01T00:00Z", &repeated));
        for error in [missing.unwrap_err(), duplicate.unwrap_err()] {
            assert!(error.to_string().len() < 200, "{}", error.to_string().len());
            assert!(format!("{error:?}").len() < 200);
        }
    }

    #[test]
    fn error_messages_never_echo_input_values() {
        let err = parse(&document(
            "2026-01-01T00:00Z",
            &attack("\"secret-value\"", "0"),
        ))
        .unwrap_err();
        assert!(!err.to_string().contains("secret-value"));
        assert!(!format!("{err:?}").contains("secret-value"));
    }

    #[test]
    fn empty_and_invalid_input_are_classified_separately() {
        assert_eq!(parse("").unwrap_err(), CharacterParseError::Truncated);

        let err = parse("{ nope }").unwrap_err();
        assert_eq!(err, CharacterParseError::InvalidJson);
        assert!(!err.is_transient());
    }

    #[test]
    fn json_errors_convert_by_category_without_keeping_their_text() {
        let eof = serde_json::from_str::<Value>("{").unwrap_err();
        let syntax = serde_json::from_str::<Value>("{ nope }").unwrap_err();
        let data = serde_json::from_str::<u8>(r#""secret-value""#).unwrap_err();
        assert_eq!(
            CharacterParseError::from(eof),
            CharacterParseError::Truncated
        );
        assert_eq!(
            CharacterParseError::from(syntax),
            CharacterParseError::InvalidJson
        );
        let converted = CharacterParseError::from(data);
        assert_eq!(converted, CharacterParseError::UnexpectedShape);
        assert!(!converted.to_string().contains("secret-value"));
        assert!(!format!("{converted:?}").contains("secret-value"));
    }
}
