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
    // Failure messages are fixed text: the parsed value, its error and the
    // warnings can contain skill names or other export content, which must not
    // reach test output even when the check fails.
    let Ok(bytes) = std::fs::read(path) else {
        panic!("the export file could not be read (details withheld)");
    };
    let Ok(export) = parse_character_export(&bytes) else {
        panic!("the export did not import (details withheld)");
    };

    println!(
        "imported {} skills, {} warnings, plugin version {}",
        export.skills().len(),
        export.warnings().len(),
        printable_version(export.plugin_version())
    );
    assert!(
        export.warnings().is_empty(),
        "the import produced {} warnings (details withheld)",
        export.warnings().len()
    );
}

/// The plugin version if it looks like a version number, else a placeholder,
/// so arbitrary text from the file is never printed.
fn printable_version(version: Option<&str>) -> &str {
    match version {
        Some(text)
            if (1..=16).contains(&text.len())
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.') =>
        {
            text
        }
        Some(_) => "<unrecognised format>",
        None => "<none>",
    }
}
