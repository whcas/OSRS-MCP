//! Public-API integration tests of the `RuneLite` `character.json` importer
//! against the full synthetic fixture. Focused parsing, validation and error
//! tests live next to the implementation in `src/runelite/character.rs` and
//! `src/runelite/timestamp.rs`. No authentic export is read by any test here.

use osrs_mcp::runelite::{CharacterParseError, Skill, parse_character_export};

const SYNTHETIC: &[u8] = include_bytes!("fixtures/runelite_export/character.synthetic.json");

#[test]
fn the_synthetic_fixture_parses_into_levels_and_experience() {
    let export = parse_character_export(SYNTHETIC).expect("synthetic fixture is valid");

    assert_eq!(export.skills().len(), 24);
    let attack = export.skill(&Skill::Attack).expect("attack present");
    assert_eq!((attack.real_level, attack.experience), (40, 37_224));
    let mining = export.skill(&Skill::Mining).expect("mining present");
    assert_eq!((mining.real_level, mining.experience), (99, 13_034_431));
    assert!(export.skill(&Skill::Sailing).is_some());
    assert_eq!(export.warnings(), []);
    assert_eq!(export.plugin_version(), Some("0.6.0"));
}

#[test]
fn the_export_time_keeps_its_original_text_and_a_parsed_instant() {
    let export = parse_character_export(SYNTHETIC).expect("valid");
    let at = export.exported_at();
    assert_eq!(at.text(), "2026-01-01T12:00:00.123456Z");
    assert_eq!(at.timestamp().as_second(), 1_767_268_800);
    assert_eq!(at.timestamp().subsec_nanosecond(), 123_456_000);
}

#[test]
fn identifiers_are_not_retained_in_the_parsed_value() {
    let export = parse_character_export(SYNTHETIC).expect("valid");
    let debug = format!("{export:?}");
    assert!(!debug.contains("Synthetic Account"));
    assert!(!debug.contains("synthetic-session"));
}

#[test]
fn a_half_written_fixture_is_reported_as_truncated_and_retryable() {
    let half = &SYNTHETIC[..SYNTHETIC.len() / 2];
    let err = parse_character_export(half).unwrap_err();
    assert_eq!(err, CharacterParseError::Truncated);
    assert!(err.is_transient());
}
