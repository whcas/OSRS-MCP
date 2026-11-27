//! Import of `RuneLite` account exports.
//!
//! Supports `character.json` ([`parse_character_export`]) and bank, inventory
//! and equipment exports ([`parse_container_export`]) from the Character State
//! Exporter plugin. The formats are recorded in `docs/research/`.
//!
//! The parser reads bytes it is given. It performs no file or network I/O and
//! has no knowledge of where exports live, so it works on synthetic fixtures
//! and on authentic exports alike. [`LocalExportSource`] is the separate,
//! bounded reader that supplies those bytes from a configured directory.

mod character;
mod container;
mod experience;
mod skill;
mod source;
mod timestamp;

pub use character::{
    CharacterExport, CharacterParseError, SkillRecord, SkillValueProblem, Warning,
    parse_character_export,
};
pub use container::{
    ContainerKind, ContainerParseError, ContainerRow, ContainerSnapshot, ContainerWarning,
    MAX_CONTAINER_EXPORT_BYTES, MAX_CONTAINER_ROWS, parse_container_export,
};
pub use experience::{MAX_EXPERIENCE, MAX_LEVEL, level_for_experience};
pub use skill::Skill;
pub use source::{
    CHARACTER_FILE_NAME, EXPORT_DIR_VAR, ExportReadError, LocalExportSource,
    MAX_CHARACTER_EXPORT_BYTES,
};
pub use timestamp::ExportedAt;
