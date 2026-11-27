//! Opt-in check of the Wiki export importer against a file you supply.
//!
//! It reads one `Special:Export` XML file from your own disk and reports
//! counts only. It never runs by default or in CI. Both gates must be open:
//!
//! 1. the test is `#[ignore]`d, so `cargo test` skips it;
//! 2. it returns immediately unless `OSRS_MCP_WIKI_EXPORT_SAMPLE` names a file.
//!
//! Run it deliberately and by hand:
//!
//! ```sh
//! OSRS_MCP_WIKI_EXPORT_SAMPLE=/path/to/export.xml \
//!     cargo test --locked --test wiki_export_sample -- --ignored --nocapture
//! ```
//!
//! The output is the number of pages imported, redirects and skipped pages
//! (the latter by reason), and the error that ended the import, if any. It
//! prints no titles, text or paths, and failure messages are fixed text, so
//! the output is safe to paste into an issue. Nothing is written to disk.
//!
//! Use only an export you are permitted to process locally. The file is never
//! committed, and licensing questions about real Wiki content stay open (see
//! ADR-0002).

use std::fs::File;
use std::io::BufReader;

use osrs_mcp::wiki::{ExportReader, ImportLimits};

const SAMPLE_VAR: &str = "OSRS_MCP_WIKI_EXPORT_SAMPLE";

#[test]
#[ignore = "reads a local export file; run with OSRS_MCP_WIKI_EXPORT_SAMPLE set and --ignored"]
fn a_local_export_imports_without_a_stream_error() {
    let Some(path) = std::env::var_os(SAMPLE_VAR) else {
        println!("skipped: set {SAMPLE_VAR} to the path of an export file to run this check");
        return;
    };
    let Ok(file) = File::open(path) else {
        panic!("the file named by {SAMPLE_VAR} could not be opened");
    };

    let mut reader = ExportReader::open(BufReader::new(file), ImportLimits::default())
        .unwrap_or_else(|error| panic!("the export header was refused: {error}"));
    println!("schema: export 0.11 (supported)");

    // Only the summary is kept, so the memory used does not grow with the file.
    reader.by_ref().for_each(drop);
    let summary = reader.summary();

    println!("pages imported: {}", summary.pages_imported);
    println!("redirects among them: {}", summary.redirects_imported);
    println!("pages skipped: {}", summary.total_pages_skipped());
    for (kind, count) in &summary.pages_skipped {
        println!("  {kind}: {count}");
    }
    if let Some(error) = &summary.stream_error {
        println!("import ended early: {error}");
    }

    assert!(summary.stream_error.is_none(), "the import ended early");
    assert!(summary.pages_imported > 0, "no page was imported");
}
