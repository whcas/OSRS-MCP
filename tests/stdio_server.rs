//! End-to-end tests that run `osrs-mcp serve` as a child process and speak raw
//! newline-delimited JSON-RPC over its stdio, as an MCP client would.
//!
//! These prove that stdout carries nothing but protocol messages and that the
//! server exits cleanly when the client closes its input.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A running server process, killed on drop so a failing test cannot leak it.
struct ServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_lines: Receiver<String>,
    stderr: Receiver<String>,
}

impl ServerProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_osrs-mcp"))
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("binary should start");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr_pipe = child.stderr.take().expect("piped stderr");

        let (line_tx, stdout_lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let (err_tx, stderr) = mpsc::channel();
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr_pipe.read_to_string(&mut text);
            let _ = err_tx.send(text);
        });

        Self {
            child,
            stdin,
            stdout_lines,
            stderr,
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        writeln!(stdin, "{message}").expect("write request");
        stdin.flush().expect("flush request");
    }

    /// Reads the next stdout line, asserting that it is a JSON-RPC 2.0 message.
    fn receive(&self) -> Value {
        let line = self
            .stdout_lines
            .recv_timeout(TIMEOUT)
            .expect("server should answer in time");
        let message: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line:?}"));
        assert_eq!(message["jsonrpc"], "2.0", "not JSON-RPC: {line:?}");
        message
    }

    /// Closes stdin and waits for exit; returns the status and remaining stdout.
    fn close_and_wait(&mut self) -> (ExitStatus, Vec<String>, String) {
        drop(self.stdin.take());
        let status = wait_with_timeout(&mut self.child);
        let remaining: Vec<String> = self.stdout_lines.try_iter().collect();
        let stderr = self.stderr.recv_timeout(TIMEOUT).unwrap_or_default();
        (status, remaining, stderr)
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_with_timeout(child: &mut Child) -> ExitStatus {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "server did not exit after stdin closed"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "stdio-test", "version": "0.0.0" }
        }
    })
}

#[test]
fn serves_server_info_over_stdio_with_a_clean_stdout() {
    let mut server = ServerProcess::start();

    server.send(&initialize_request());
    let init = server.receive();
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "osrs-mcp");

    server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let tools = server.receive();
    assert_eq!(tools["id"], 2);
    assert_eq!(tools["result"]["tools"][0]["name"], "server_info");

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "server_info", "arguments": {} }
    }));
    let call = server.receive();
    assert_eq!(call["id"], 3);
    assert_eq!(
        call["result"]["structuredContent"]["version"],
        env!("CARGO_PKG_VERSION")
    );

    let (status, remaining, stderr) = server.close_and_wait();
    assert!(
        status.success(),
        "exit status: {status:?}, stderr: {stderr}"
    );
    assert!(remaining.is_empty(), "unexpected stdout: {remaining:?}");
}

#[test]
fn diagnostics_go_to_stderr_and_never_to_stdout() {
    let mut server = ServerProcess::start();

    server.send(&initialize_request());
    let _ = server.receive();

    let (status, remaining, stderr) = server.close_and_wait();
    assert!(status.success(), "exit status: {status:?}");
    assert!(
        stderr.contains("serving MCP on stdio"),
        "startup diagnostic should be on stderr: {stderr:?}"
    );
    assert!(remaining.is_empty(), "unexpected stdout: {remaining:?}");
}
