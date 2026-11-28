//! Command-line entry point.
//!
//! Stdout is reserved for program output and MCP protocol traffic; usage and
//! diagnostics go to stderr. In particular `serve` must never write anything
//! but protocol messages to stdout.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;
use osrs_mcp::BuildInfo;
use osrs_mcp::wiki::commands::{self, CommandError};

use osrs_mcp::cli::{self, Cli, Command, WikiCommand};

/// Exit code for incorrect command-line usage, which is also what the parser
/// reports for its own errors.
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let info = osrs_mcp::build_info();

    // `args_os` rather than `args`: the latter panics on non-Unicode input.
    let cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            // Best effort: there is nowhere left to report a failure to write.
            let _ = error.print();
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(USAGE_ERROR));
        }
    };
    match cli.command {
        Command::Serve => run_server(info),
        Command::Wiki { command } => run_wiki(command),
    }
}

/// Renders an error followed by its chain of causes, for example
/// `MCP initialization failed: connection closed`.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    text
}

/// Runs the MCP server on stdio, reporting progress and failures on stderr.
fn run_server(info: BuildInfo) -> ExitCode {
    let mut stderr = io::stderr().lock();
    // Best effort: diagnostics must never take the server down.
    let _ = writeln!(
        stderr,
        "{} {} serving MCP on stdio",
        info.name, info.version
    );

    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start async runtime: {error}"))
        .and_then(|runtime| {
            runtime
                .block_on(osrs_mcp::mcp::serve_stdio())
                .map_err(|error| describe(&error))
        });

    match result {
        Ok(()) => {
            let _ = writeln!(stderr, "{} stopped: client disconnected", info.name);
            ExitCode::SUCCESS
        }
        Err(message) => {
            let _ = writeln!(stderr, "{} failed: {message}", info.name);
            ExitCode::FAILURE
        }
    }
}

/// Runs a Wiki command: the result goes to stdout, failures to stderr.
fn run_wiki(command: WikiCommand) -> ExitCode {
    match command {
        WikiCommand::Import(args) => {
            finish(commands::import(&args.request()).map(|report| cli::render::import(&report)))
        }
        WikiCommand::Search(args) => match args.request() {
            Ok(request) => {
                finish(commands::search(&request).map(|report| cli::render::search(&report)))
            }
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "osrs-mcp: {error}");
                ExitCode::from(USAGE_ERROR)
            }
        },
        WikiCommand::Page(args) => match commands::page(&args.request()) {
            Ok(report) => match cli::render::page(&report) {
                Ok(text) => print(&text),
                Err(message) => {
                    let _ = writeln!(io::stderr().lock(), "osrs-mcp: {message}");
                    ExitCode::FAILURE
                }
            },
            Err(error) => failure(&error),
        },
    }
}

/// Prints a successful result, or reports the failure.
fn finish(result: Result<String, CommandError>) -> ExitCode {
    match result {
        Ok(text) => print(&text),
        Err(error) => failure(&error),
    }
}

fn print(text: &str) -> ExitCode {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn failure(error: &CommandError) -> ExitCode {
    // Best effort: there is nowhere left to report a failure to write.
    let _ = writeln!(io::stderr().lock(), "osrs-mcp: {}", describe(error));
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_appends_the_cause_of_a_serve_error() {
        let error =
            osrs_mcp::mcp::ServeError::Initialize(Box::new(io::Error::other("pipe closed")));
        assert_eq!(describe(&error), "MCP initialization failed: pipe closed");
    }

    #[test]
    fn describe_appends_the_cause_of_a_command_error_without_a_path() {
        let error = CommandError::ExportFile(io::Error::other("os says /secret/path"));
        assert_eq!(
            describe(&error),
            "the export file could not be opened: os says /secret/path"
        );
    }
}
