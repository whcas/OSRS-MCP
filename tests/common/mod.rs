//! Setup shared by the in-process MCP tests: a real rmcp client talking to a
//! server over an in-memory pipe, reading synthetic exports from temporary
//! directories.

// `unreachable_pub` rejects `pub` here and this lint rejects `pub(crate)`, and
// a test module is the one place that needs crate-wide visibility without
// being part of any API.
#![expect(
    clippy::redundant_pub_crate,
    reason = "unreachable_pub forbids plain `pub` in a test-crate module, so `pub(crate)` is the only form"
)]

use std::fs;

use osrs_mcp::mcp::OsrsMcpServer;
use osrs_mcp::runelite::LocalExportSource;
use rmcp::{
    ServiceExt,
    model::CallToolResult,
    service::{RoleClient, RunningService},
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::task::JoinHandle;

pub(crate) type Client = RunningService<RoleClient, ()>;

/// Connects a client to a fresh server reading exports from `source`,
/// completing the MCP handshake.
pub(crate) async fn connect(source: LocalExportSource) -> (Client, JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server = tokio::spawn(async move {
        let running = OsrsMcpServer::new()
            .expect("server should be configured")
            .with_export_source(source)
            .serve(server_io)
            .await
            .expect("server should initialise");
        running.waiting().await.expect("clean shutdown");
    });
    let client = ().serve(client_io).await.expect("client should initialise");
    (client, server)
}

/// A temporary directory holding `content` as `file`, with a source reading it.
/// `prefix` names the directory after the calling test file.
pub(crate) fn source_in(
    prefix: &str,
    file: &str,
    content: impl AsRef<[u8]>,
) -> (TempDir, LocalExportSource) {
    let dir = TempDir::with_prefix(prefix).unwrap();
    fs::write(dir.path().join(file), content).unwrap();
    let source = LocalExportSource::new(dir.path().to_path_buf());
    (dir, source)
}

/// The error text of a failed call, parsed as JSON.
pub(crate) fn failure_json(result: &CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(true), "result: {result:?}");
    assert!(
        result.structured_content.is_none(),
        "failures must not carry structured content"
    );
    let text = &result.content[0].as_text().expect("failure text").text;
    serde_json::from_str(text).expect("failure text is JSON")
}
