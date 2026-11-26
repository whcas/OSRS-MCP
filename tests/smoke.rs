//! Smoke tests that run the compiled binary end to end.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_osrs-mcp"))
        .args(args)
        .output()
        .expect("binary should start")
}

#[test]
fn version_flag_prints_name_and_version_to_stdout() {
    let output = run(&["--version"]);

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert_eq!(stdout, format!("osrs-mcp {}\n", env!("CARGO_PKG_VERSION")));
    assert!(output.stderr.is_empty(), "stderr should be empty");
}

#[test]
fn unknown_argument_fails_with_usage_on_stderr_only() {
    let output = run(&["--no-such-flag"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "stdout must stay clean for protocol output"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("Usage:"), "stderr: {stderr}");
}

#[test]
fn no_arguments_fails_with_usage_on_stderr_only() {
    let output = run(&[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "stdout must stay clean for protocol output"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("Usage:"), "stderr: {stderr}");
}
