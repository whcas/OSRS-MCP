//! Command-line entry point.
//!
//! Stdout is reserved for program output (and, later, MCP protocol traffic);
//! usage and diagnostics go to stderr.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use osrs_mcp::BuildInfo;

/// Exit code for incorrect command-line usage.
const USAGE_ERROR: u8 = 2;

/// What the user asked the binary to do.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Print the name and version to stdout.
    Version,
    /// Arguments were missing or not recognised.
    Usage,
}

fn main() -> ExitCode {
    let info = osrs_mcp::build_info();

    // `args_os` rather than `args`: the latter panics on non-Unicode input.
    match parse_command(std::env::args_os().skip(1)) {
        Command::Version => match write_version(&mut io::stdout().lock(), info) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        },
        Command::Usage => {
            // Best effort: there is nowhere left to report a failure to write
            // to stderr, and the exit code still signals the usage error.
            let _ = write_usage(&mut io::stderr().lock(), info);
            ExitCode::from(USAGE_ERROR)
        }
    }
}

/// Maps command-line arguments (excluding the program name) to a command.
fn parse_command(args: impl IntoIterator<Item = OsString>) -> Command {
    let mut args = args.into_iter();
    match (args.next(), args.next()) {
        (Some(flag), None) if flag == "--version" || flag == "-V" => Command::Version,
        _ => Command::Usage,
    }
}

/// Writes the version line, for example `osrs-mcp 0.1.0`.
fn write_version(out: &mut impl Write, info: BuildInfo) -> io::Result<()> {
    writeln!(out, "{} {}", info.name, info.version)
}

/// Writes the usage message shown when arguments are missing or invalid.
fn write_usage(out: &mut impl Write, info: BuildInfo) -> io::Result<()> {
    writeln!(
        out,
        "{name} {version}\n\nUsage: {name} --version\n\nNo server is implemented yet.",
        name = info.name,
        version = info.version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: BuildInfo = BuildInfo {
        name: "osrs-mcp",
        version: "1.2.3",
    };

    fn parse(args: &[&str]) -> Command {
        parse_command(args.iter().map(OsString::from))
    }

    #[test]
    fn version_flags_select_version() {
        assert_eq!(parse(&["--version"]), Command::Version);
        assert_eq!(parse(&["-V"]), Command::Version);
    }

    #[test]
    fn missing_extra_or_unknown_arguments_select_usage() {
        assert_eq!(parse(&[]), Command::Usage);
        assert_eq!(parse(&["--help"]), Command::Usage);
        assert_eq!(parse(&["--version", "extra"]), Command::Usage);
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_argument_selects_usage() {
        use std::os::unix::ffi::OsStringExt;

        let args = [OsString::from_vec(b"--\xff".to_vec())];
        assert_eq!(parse_command(args), Command::Usage);
    }

    #[test]
    fn version_output_is_name_and_version() {
        let mut out = Vec::new();
        write_version(&mut out, INFO).unwrap();
        assert_eq!(out, b"osrs-mcp 1.2.3\n");
    }

    #[test]
    fn usage_output_names_the_supported_flag() {
        let mut out = Vec::new();
        write_usage(&mut out, INFO).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Usage: osrs-mcp --version"), "usage: {text}");
    }
}
