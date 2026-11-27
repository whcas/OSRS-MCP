//! Import of `RuneLite` account exports.
//!
//! Supports `character.json` ([`parse_character_export`]), bank, inventory and
//! equipment exports ([`parse_container_export`]) and `quests.json`
//! ([`parse_quest_export`]) from the Character State Exporter plugin. The
//! formats are recorded in `docs/research/`.
//!
//! The parser reads bytes it is given. It performs no file or network I/O and
//! has no knowledge of where exports live, so it works on synthetic fixtures
//! and on authentic exports alike. [`LocalExportSource`] is the separate,
//! bounded reader that supplies those bytes from a configured directory.

mod character;
mod container;
mod experience;
mod quest;
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
pub use quest::{
    MAX_QUEST_EXPORT_BYTES, MAX_QUEST_ROWS, QuestParseError, QuestProgress, QuestRecord,
    QuestState, QuestWarning, RuneLiteQuestId, parse_quest_export,
};
pub use skill::Skill;
pub use source::{
    BANK_FILE_NAME, CHARACTER_FILE_NAME, EQUIPMENT_FILE_NAME, EXPORT_DIR_VAR, ExportReadError,
    INVENTORY_FILE_NAME, LocalExportSource, MAX_CHARACTER_EXPORT_BYTES, ParseFailure,
    QUEST_FILE_NAME, container_file_name,
};
pub use timestamp::ExportedAt;
