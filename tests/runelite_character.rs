//! Behaviour of the `RuneLite` `character.json` importer, using only synthetic
//! input. No authentic export is read by any test.

use osrs_mcp::runelite::{
    CharacterParseError, ExportedAt, Skill, SkillValueProblem, Warning, parse_character_export,
};

const SYNTHETIC: &[u8] = include_bytes!("fixtures/runelite_export/character.synthetic.json");

/// A minimal valid document with one `Attack` skill, with `stats_body` spliced
/// in so each test states only what it varies.
fn document(exported_at: &str, stats_body: &str) -> String {
    format!(r#"{{"exported_at":"{exported_at}","stats":{{{stats_body}}}}}"#)
}

fn attack(real_level: &str, experience: &str) -> String {
    format!(r#""Attack":{{"real_level":{real_level},"experience":{experience}}}"#)
}

fn parse(text: &str) -> Result<osrs_mcp::runelite::CharacterExport, CharacterParseError> {
    parse_character_export(text.as_bytes())
}

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
fn unknown_fields_are_ignored_at_every_level() {
    let text = r#"{"exported_at":"2026-01-01T00:00Z","future":{"a":1},
        "stats":{"Attack":{"real_level":1,"experience":0,"boosted_level":1,"extra":[1,2]}}}"#;
    let export = parse(text).expect("unknown fields are ignored");
    assert_eq!(export.skills().len(), 1);
}

#[test]
fn timestamps_in_every_form_java_can_print_are_accepted() {
    let cases = [
        ("2026-01-01T12:00Z", 1_767_268_800, 0),
        ("2026-01-01T12:00:30Z", 1_767_268_830, 0),
        ("2026-01-01T12:00:00.5Z", 1_767_268_800, 500_000_000),
        ("2026-01-01T12:00:00.123Z", 1_767_268_800, 123_000_000),
        ("2026-01-01T12:00:00.123456789Z", 1_767_268_800, 123_456_789),
        ("2026-01-01T13:00:00+01:00", 1_767_268_800, 0),
        ("2026-01-01T07:00:00-05:00", 1_767_268_800, 0),
        ("2024-02-29T00:00Z", 1_709_164_800, 0),
        ("1970-01-01T00:00Z", 0, 0),
    ];
    for (text, seconds, nanos) in cases {
        let at = ExportedAt::parse(text).unwrap_or_else(|| panic!("{text} should parse"));
        assert_eq!(at.timestamp().as_second(), seconds, "{text}");
        assert_eq!(at.timestamp().subsec_nanosecond(), nanos, "{text}");
        assert_eq!(at.text(), text);
    }
}

#[test]
fn malformed_timestamps_are_rejected() {
    let cases = [
        "",
        "2026-01-01",
        "2026-01-01T12:00",
        "2026-13-01T12:00Z",
        "2026-02-29T12:00Z",
        "2026-01-32T12:00Z",
        "2026-01-01T24:00Z",
        "2026-01-01T12:60Z",
        "2026-01-01T12:00:00.Z",
        "2026-01-01T12:00:00.1234567890Z",
        "2026-01-01T12:00Zjunk",
        "2026-01-01T12:00:00 Z",
        "2026-01-01T12:00:00[UTC]",
    ];
    for text in cases {
        assert!(
            ExportedAt::parse(text).is_none(),
            "{text:?} should be rejected"
        );
    }
}

#[test]
fn a_leap_second_is_normalised_to_the_previous_second() {
    let leap = ExportedAt::parse("2026-01-01T12:00:60Z").expect("accepted");
    let normal = ExportedAt::parse("2026-01-01T12:00:59Z").expect("valid");
    assert_eq!(leap.timestamp(), normal.timestamp());
    assert_eq!(leap.timestamp().as_second(), 1_767_268_859);
    // The original text is still the text that was exported.
    assert_eq!(leap.text(), "2026-01-01T12:00:60Z");
}

#[test]
fn lenient_forms_the_plugin_never_writes_are_accepted_and_documented() {
    // These are accepted by the underlying parser; see `ExportedAt::parse`.
    for text in [
        "2026-01-01 12:00:00Z",
        "2026-01-01t12:00:00z",
        "20260101T120000Z",
        "2026-01-01T12:00:00+25:00",
    ] {
        assert!(ExportedAt::parse(text).is_some(), "{text:?} is accepted");
    }
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
fn truncated_input_is_distinguished_from_invalid_input() {
    let whole = String::from_utf8(SYNTHETIC.to_vec()).expect("utf8");
    let cut = &whole[..whole.len() / 2];
    let err = parse(cut).unwrap_err();
    assert_eq!(err, CharacterParseError::Truncated);
    assert!(err.is_transient());

    assert_eq!(parse("").unwrap_err(), CharacterParseError::Truncated);

    let err = parse("{ nope }").unwrap_err();
    assert_eq!(err, CharacterParseError::InvalidJson);
    assert!(!err.is_transient());
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
fn the_plugin_version_is_recorded_as_text_and_any_value_is_accepted() {
    let text = r#"{"exported_at":"2026-01-01T00:00Z","plugin_version":"9.9.9-future",
        "stats":{"Attack":{"real_level":1,"experience":0}}}"#;
    assert_eq!(
        parse(text).expect("valid").plugin_version(),
        Some("9.9.9-future")
    );
    let without =
        r#"{"exported_at":"2026-01-01T00:00Z","stats":{"Attack":{"real_level":1,"experience":0}}}"#;
    assert_eq!(parse(without).expect("valid").plugin_version(), None);
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
