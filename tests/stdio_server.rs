//! End-to-end tests that run `osrs-mcp serve` as a child process and speak raw
//! newline-delimited JSON-RPC over its stdio, as an MCP client would.
//!
//! These prove that stdout carries nothing but protocol messages and that the
//! server exits cleanly when the client closes its input.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A running server process, killed on drop so a failing test cannot leak it.
///
/// Both output pipes are read by dedicated threads that forward every line (or
/// read error) over a channel and drop their sender at end of file. A
/// disconnected channel therefore means the pipe reached EOF, which lets
/// `close_and_wait` account for every byte the server wrote.
struct ServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<io::Result<String>>,
    stderr: Receiver<io::Result<String>>,
}

/// Forwards each line of `pipe` to the returned channel until EOF or error.
fn forward_lines(pipe: impl Read + Send + 'static) -> Receiver<io::Result<String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            let failed = line.is_err();
            if sender.send(line).is_err() || failed {
                break;
            }
        }
    });
    receiver
}

/// Collects everything left on `lines` until EOF, within a bounded wait.
fn drain_to_eof(lines: &Receiver<io::Result<String>>, name: &str) -> Vec<String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut collected = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(remaining) {
            Ok(Ok(line)) => collected.push(line),
            Ok(Err(error)) => panic!("reading {name} failed: {error}"),
            Err(RecvTimeoutError::Disconnected) => return collected,
            Err(RecvTimeoutError::Timeout) => panic!("{name} did not reach EOF in time"),
        }
    }
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
        let stdout = forward_lines(child.stdout.take().expect("piped stdout"));
        let stderr = forward_lines(child.stderr.take().expect("piped stderr"));

        Self {
            child,
            stdin,
            stdout,
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
            .stdout
            .recv_timeout(TIMEOUT)
            .expect("server should answer in time")
            .expect("stdout should be readable");
        parse_json_rpc(&line)
    }

    /// Closes stdin, waits for exit and for both pipes to reach EOF.
    ///
    /// Returns the exit status, every JSON-RPC message still on stdout (each
    /// line is validated, so stray non-protocol output fails here) and all of
    /// stderr.
    fn close_and_wait(&mut self) -> (ExitStatus, Vec<Value>, String) {
        drop(self.stdin.take());
        let status = wait_with_timeout(&mut self.child);
        let leftover = drain_to_eof(&self.stdout, "stdout")
            .iter()
            .map(|line| parse_json_rpc(line))
            .collect();
        let stderr = drain_to_eof(&self.stderr, "stderr").join("\n");
        (status, leftover, stderr)
    }
}

/// Parses a stdout line, failing the test unless it is a JSON-RPC 2.0 message.
fn parse_json_rpc(line: &str) -> Value {
    let message: Value = serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line:?}"));
    assert_eq!(message["jsonrpc"], "2.0", "not JSON-RPC: {line:?}");
    message
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_with_timeout(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
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
    assert!(
        tools["result"]["tools"]
            .as_array()
            .is_some_and(|list| list.iter().any(|tool| tool["name"] == "server_info")),
        "server_info should be listed: {tools}"
    );

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

#[test]
fn malformed_input_is_skipped_without_polluting_stdout() {
    let mut server = ServerProcess::start();

    // Not JSON at all, before the handshake, then a truncated object after it.
    server
        .stdin
        .as_mut()
        .expect("stdin open")
        .write_all(b"this is not json\n")
        .expect("write garbage");
    server.send(&initialize_request());
    assert_eq!(server.receive()["id"], 1);
    server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server
        .stdin
        .as_mut()
        .expect("stdin open")
        .write_all(b"{\"broken\":\n")
        .expect("write truncated JSON");
    server.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let tools = server.receive();
    assert_eq!(tools["id"], 2, "server should keep serving: {tools}");
    assert!(
        tools["result"]["tools"]
            .as_array()
            .is_some_and(|list| list.iter().any(|tool| tool["name"] == "server_info")),
        "server_info should be listed: {tools}"
    );

    let (status, leftover, stderr) = server.close_and_wait();
    assert!(
        status.success(),
        "exit status: {status:?}, stderr: {stderr}"
    );
    assert!(leftover.is_empty(), "unexpected stdout: {leftover:?}");
}
