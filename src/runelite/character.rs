//! Parsing of the Character State Exporter's `character.json`.
//!
//! Decisions are in `docs/research/runelite-export-formats.md`. In short:
//! unknown fields are ignored, `boosted_level`, `session_id` and
//! `account_name` are never read or retained, a level that disagrees with
//! experience is a warning, and error values never contain input values.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::ops::RangeInclusive;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

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

    /// The plugin version recorded in the file, if any. Any text is accepted.
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

/// Why `character.json` could not be imported. No variant carries an input
/// value other than a skill name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharacterParseError {
    /// The input ended inside a JSON value, or was empty. The plugin does not
    /// write atomically, so this usually means a write was in progress: retry.
    Truncated,
    /// The input is not valid JSON.
    InvalidJson,
    /// Valid JSON of an unexpected shape, for example not an object.
    UnexpectedShape,
    /// A required top-level field is absent.
    MissingField {
        /// The field name.
        field: &'static str,
    },
    /// `exported_at` is not a timestamp the plugin writes.
    InvalidTimestamp,
    /// `plugin_version` is present but not a string.
    InvalidPluginVersion,
    /// `stats` is an empty object.
    NoSkills,
    /// A skill appears twice in `stats`.
    DuplicateSkill {
        /// The skill name.
        name: String,
    },
    /// A skill lacks a required value.
    MissingSkillField {
        /// The skill name.
        skill: String,
        /// The missing field.
        field: &'static str,
    },
    /// A skill value is not acceptable.
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

impl fmt::Display for CharacterParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => {
                f.write_str("export ended unexpectedly (a write may be in progress)")
            }
            Self::InvalidJson => f.write_str("export is not valid JSON"),
            Self::UnexpectedShape => f.write_str("export JSON has an unexpected shape"),
            Self::MissingField { field } => write!(f, "required field `{field}` is missing"),
            Self::InvalidTimestamp => f.write_str("`exported_at` is not a recognised timestamp"),
            Self::InvalidPluginVersion => f.write_str("`plugin_version` is not a string"),
            Self::NoSkills => f.write_str("`stats` contains no skills"),
            Self::DuplicateSkill { name } => write!(f, "skill `{name}` appears more than once"),
            Self::MissingSkillField { skill, field } => {
                write!(f, "skill `{skill}` is missing `{field}`")
            }
            Self::InvalidSkillValue {
                skill,
                field,
                problem,
            } => {
                let reason = match problem {
                    SkillValueProblem::NotAnInteger => "is not a non-negative integer",
                    SkillValueProblem::OutOfRange => "is out of range",
                };
                write!(f, "skill `{skill}` field `{field}` {reason}")
            }
        }
    }
}

impl std::error::Error for CharacterParseError {}

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
    let fields: BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(bytes).map_err(|error| classify_json_error(&error))?;

    let exported_at = match read_value(&fields, "exported_at")? {
        None => {
            return Err(CharacterParseError::MissingField {
                field: "exported_at",
            });
        }
        Some(Value::String(text)) => {
            ExportedAt::parse(&text).ok_or(CharacterParseError::InvalidTimestamp)?
        }
        Some(_) => return Err(CharacterParseError::InvalidTimestamp),
    };
    let plugin_version = match read_value(&fields, "plugin_version")? {
        None | Some(Value::Null) => None,
        Some(Value::String(version)) => Some(version),
        Some(_) => return Err(CharacterParseError::InvalidPluginVersion),
    };
    let stats = fields
        .get("stats")
        .ok_or(CharacterParseError::MissingField { field: "stats" })?;
    let SkillEntries(entries) =
        serde_json::from_str(stats.get()).map_err(|error| classify_json_error(&error))?;
    let (skills, warnings) = read_skills(&entries)?;

    Ok(CharacterExport {
        exported_at,
        plugin_version,
        skills,
        warnings,
    })
}

/// A top-level field as a value, or `None` if the file has no such field. An
/// explicit JSON `null` is `Some(Value::Null)`, not missing.
fn read_value(
    fields: &BTreeMap<String, Box<RawValue>>,
    name: &str,
) -> Result<Option<Value>, CharacterParseError> {
    fields
        .get(name)
        .map(|raw| serde_json::from_str(raw.get()))
        .transpose()
        .map_err(|error| classify_json_error(&error))
}

fn classify_json_error(error: &serde_json::Error) -> CharacterParseError {
    match error.classify() {
        serde_json::error::Category::Eof => CharacterParseError::Truncated,
        serde_json::error::Category::Syntax | serde_json::error::Category::Io => {
            CharacterParseError::InvalidJson
        }
        serde_json::error::Category::Data => CharacterParseError::UnexpectedShape,
    }
}

fn read_skills(
    entries: &[(String, Value)],
) -> Result<(Vec<SkillRecord>, Vec<Warning>), CharacterParseError> {
    if entries.is_empty() {
        return Err(CharacterParseError::NoSkills);
    }
    let mut seen = HashSet::new();
    let mut skills = Vec::with_capacity(entries.len());
    let mut warnings = Vec::new();

    for (name, value) in entries {
        if !seen.insert(name.as_str()) {
            return Err(CharacterParseError::DuplicateSkill { name: name.clone() });
        }
        let record = read_skill(name, value)?;
        warnings.extend(warning_for(&record));
        skills.push(record);
    }
    Ok((skills, warnings))
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
        skill: skill.to_owned(),
        field,
        problem,
    };
    let value = value.ok_or_else(|| CharacterParseError::MissingSkillField {
        skill: skill.to_owned(),
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
