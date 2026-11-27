//! Import of `RuneLite` account exports.
//!
//! Currently supports `character.json` from the Character State Exporter
//! plugin ([`parse_character_export`]). The format and the decisions behind
//! this module are recorded in `docs/research/runelite-export-formats.md`.
//!
//! The parser reads bytes it is given. It performs no file or network I/O and
//! has no knowledge of where exports live, so it works on synthetic fixtures
//! and on authentic exports alike. [`LocalExportSource`] is the separate,
//! bounded reader that supplies those bytes from a configured directory.

mod character;
mod diary;
mod experience;
mod skill;
mod source;
mod timestamp;

pub use character::{
    CharacterExport, CharacterParseError, SkillRecord, SkillValueProblem, Warning,
    parse_character_export,
};
pub use diary::{
    DiaryArea, DiaryParseError, DiaryProgress, DiaryTask, DiaryTier, DiaryTierProgress,
    DiaryWarning, MAX_DIARY_EXPORT_BYTES, parse_diary_export,
};
pub use experience::{MAX_EXPERIENCE, MAX_LEVEL, level_for_experience};
pub use skill::Skill;
pub use source::{
    CHARACTER_FILE_NAME, EXPORT_DIR_VAR, ExportReadError, LocalExportSource,
    MAX_CHARACTER_EXPORT_BYTES,
};
pub use timestamp::ExportedAt;
