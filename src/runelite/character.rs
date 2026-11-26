//! Parsing of the Character State Exporter's `character.json`.
//!
//! Decisions are in `docs/research/runelite-export-formats.md`. In short:
//! unknown fields are ignored, `boosted_level`, `session_id` and
//! `account_name` are never read or retained, a level that disagrees with
//! experience is a warning, and error values never contain input values.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

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
    let raw: RawCharacter =
        serde_json::from_slice(bytes).map_err(|error| classify_json_error(&error))?;

    let exported_at = parse_exported_at(raw.exported_at)?;
    let plugin_version = parse_plugin_version(raw.plugin_version)?;
    let stats = raw
        .stats
        .0
        .ok_or(CharacterParseError::MissingField { field: "stats" })?;
    let (skills, warnings) = build_skills(stats.0)?;

    Ok(CharacterExport {
        exported_at,
        plugin_version,
        skills,
        warnings,
    })
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

fn parse_exported_at(raw: Raw) -> Result<ExportedAt, CharacterParseError> {
    match raw.0 {
        None => Err(CharacterParseError::MissingField {
            field: "exported_at",
        }),
        Some(Value::String(text)) => {
            ExportedAt::parse(&text).ok_or(CharacterParseError::InvalidTimestamp)
        }
        Some(_) => Err(CharacterParseError::InvalidTimestamp),
    }
}

fn parse_plugin_version(raw: Raw) -> Result<Option<String>, CharacterParseError> {
    match raw.0 {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(version)) => Ok(Some(version)),
        Some(_) => Err(CharacterParseError::InvalidPluginVersion),
    }
}

fn build_skills(
    entries: Vec<(String, RawSkill)>,
) -> Result<(Vec<SkillRecord>, Vec<Warning>), CharacterParseError> {
    if entries.is_empty() {
        return Err(CharacterParseError::NoSkills);
    }
    let mut seen = HashSet::new();
    let mut skills = Vec::with_capacity(entries.len());
    let mut warnings = Vec::new();

    for (name, raw) in entries {
        if !seen.insert(name.clone()) {
            return Err(CharacterParseError::DuplicateSkill { name });
        }
        let real_level = read_level(&name, raw.real_level)?;
        let experience = read_experience(&name, raw.experience)?;
        let skill = Skill::from_name(&name);
        if let Some(warning) = check_skill(&skill, real_level, experience) {
            warnings.push(warning);
        }
        skills.push(SkillRecord {
            skill,
            real_level,
            experience,
        });
    }
    Ok((skills, warnings))
}

fn check_skill(skill: &Skill, real_level: u8, experience: u32) -> Option<Warning> {
    if !skill.is_known() {
        return Some(Warning::UnknownSkill {
            name: skill.name().to_owned(),
        });
    }
    let implied_level = level_for_experience(experience);
    (implied_level != real_level).then(|| Warning::LevelExperienceMismatch {
        skill: skill.clone(),
        recorded_level: real_level,
        implied_level,
    })
}

fn read_level(skill: &str, raw: Raw) -> Result<u8, CharacterParseError> {
    let value = read_integer(skill, "real_level", raw)?;
    u8::try_from(value)
        .ok()
        .filter(|level| (1..=MAX_LEVEL).contains(level))
        .ok_or_else(|| out_of_range(skill, "real_level"))
}

fn read_experience(skill: &str, raw: Raw) -> Result<u32, CharacterParseError> {
    let value = read_integer(skill, "experience", raw)?;
    u32::try_from(value)
        .ok()
        .filter(|experience| *experience <= MAX_EXPERIENCE)
        .ok_or_else(|| out_of_range(skill, "experience"))
}

fn read_integer(skill: &str, field: &'static str, raw: Raw) -> Result<u64, CharacterParseError> {
    let value = raw
        .0
        .ok_or_else(|| CharacterParseError::MissingSkillField {
            skill: skill.to_owned(),
            field,
        })?;
    value
        .as_u64()
        .ok_or_else(|| CharacterParseError::InvalidSkillValue {
            skill: skill.to_owned(),
            field,
            problem: SkillValueProblem::NotAnInteger,
        })
}

fn out_of_range(skill: &str, field: &'static str) -> CharacterParseError {
    CharacterParseError::InvalidSkillValue {
        skill: skill.to_owned(),
        field,
        problem: SkillValueProblem::OutOfRange,
    }
}

// Input DTOs. They keep raw JSON values so that every rejection is mapped to
// a typed error here rather than to a serde message that could echo input.

/// A field that may be absent. Unlike `Option<T>`, an explicit JSON `null` is
/// present, so it is reported as a wrong type rather than as missing.
#[derive(Default)]
struct Raw(Option<Value>);

impl<'de> Deserialize<'de> for Raw {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

/// The top-level document. Deserialised from a JSON object only: a derived
/// `Deserialize` would also accept a JSON array, filling fields by position.
struct RawCharacter {
    exported_at: Raw,
    plugin_version: Raw,
    stats: RawStatsField,
}

impl<'de> Deserialize<'de> for RawCharacter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(CharacterVisitor)
    }
}

struct CharacterVisitor;

impl<'de> Visitor<'de> for CharacterVisitor {
    type Value = RawCharacter;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a character export object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut character = RawCharacter {
            exported_at: Raw::default(),
            plugin_version: Raw::default(),
            stats: RawStatsField::default(),
        };
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "exported_at" => character.exported_at = map.next_value()?,
                "plugin_version" => character.plugin_version = map.next_value()?,
                "stats" => character.stats = map.next_value()?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(character)
    }
}

/// One skill's values. Object-only, for the same reason as [`RawCharacter`].
struct RawSkill {
    real_level: Raw,
    experience: Raw,
}

impl<'de> Deserialize<'de> for RawSkill {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(SkillVisitor)
    }
}

struct SkillVisitor;

impl<'de> Visitor<'de> for SkillVisitor {
    type Value = RawSkill;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a skill object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut skill = RawSkill {
            real_level: Raw::default(),
            experience: Raw::default(),
        };
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "real_level" => skill.real_level = map.next_value()?,
                "experience" => skill.experience = map.next_value()?,
                // Includes `boosted_level`, which is deliberately not read.
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(skill)
    }
}

/// `stats` as an ordered list so that a repeated key is detectable, which a
/// map type would hide by keeping only the last value.
#[derive(Default)]
struct RawStatsField(Option<RawStats>);

struct RawStats(Vec<(String, RawSkill)>);

impl<'de> Deserialize<'de> for RawStatsField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_map(StatsVisitor)
            .map(|stats| Self(Some(stats)))
    }
}

struct StatsVisitor;

impl<'de> Visitor<'de> for StatsVisitor {
    type Value = RawStats;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object of skills")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry::<String, RawSkill>()? {
            entries.push(entry);
        }
        Ok(RawStats(entries))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Err(E::custom("stats is null"))
    }
}
