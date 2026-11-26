//! Opt-in check of the importer against an authentic local `character.json`.
//!
//! Never runs by default or in CI: the test is `#[ignore]`d and also needs
//! `OSRS_MCP_RUNELITE_CHARACTER_JSON` to name the file. Run it by hand:
//!
//! ```sh
//! OSRS_MCP_RUNELITE_CHARACTER_JSON=/path/to/character.json \
//!     cargo test --locked --test runelite_local_export -- --ignored --nocapture
//! ```
//!
//! The file is only read. The output is limited to counts, so no account
//! name, path or experience value is printed.

use osrs_mcp::runelite::parse_character_export;

const PATH_VAR: &str = "OSRS_MCP_RUNELITE_CHARACTER_JSON";

#[test]
#[ignore = "reads a local export; set OSRS_MCP_RUNELITE_CHARACTER_JSON and use --ignored"]
fn an_authentic_local_export_imports_without_warnings_or_errors() {
    let Some(path) = std::env::var_os(PATH_VAR) else {
        println!("skipped: set {PATH_VAR} to run the local export check");
        return;
    };
    let bytes = std::fs::read(path).expect("the export file is readable");

    let export = parse_character_export(&bytes).expect("the export imports");

    println!(
        "imported {} skills, {} warnings, plugin version {:?}",
        export.skills().len(),
        export.warnings().len(),
        export.plugin_version()
    );
    assert_eq!(export.warnings(), []);
}
