//! Command-line entry point.
//!
//! Stdout is reserved for program output (and, later, MCP protocol traffic);
//! usage and diagnostics go to stderr.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

/// Exit code for incorrect command-line usage.
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let info = osrs_mcp::build_info();
    // `args_os` rather than `args`: the latter panics on non-Unicode input.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    match args.as_slice() {
        [flag] if flag == "--version" || flag == "-V" => {
            match writeln!(io::stdout(), "{} {}", info.name, info.version) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
        _ => {
            // Nothing useful to do if stderr itself is unavailable.
            let _ = writeln!(
                io::stderr(),
                "{name} {version}\n\nUsage: {name} --version\n\nNo server is implemented yet.",
                name = info.name,
                version = info.version,
            );
            ExitCode::from(USAGE_ERROR)
        }
    }
}
