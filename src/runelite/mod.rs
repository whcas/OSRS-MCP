//! Import of `RuneLite` account exports.
//!
//! Currently supports `character.json` from the Character State Exporter
//! plugin ([`parse_character_export`]). The format and the decisions behind
//! this module are recorded in `docs/research/runelite-export-formats.md`.
//!
//! This module reads bytes it is given. It performs no file or network I/O and
//! has no knowledge of where exports live, so it works on synthetic fixtures
//! and on authentic exports alike.

mod character;
mod experience;
mod skill;
mod timestamp;

pub use character::{
    CharacterExport, CharacterParseError, SkillRecord, SkillValueProblem, Warning,
    parse_character_export,
};
pub use experience::{MAX_EXPERIENCE, MAX_LEVEL, level_for_experience};
pub use skill::Skill;
pub use timestamp::ExportedAt;
