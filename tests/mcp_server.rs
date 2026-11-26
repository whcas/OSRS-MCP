//! In-process MCP protocol tests: a real rmcp client talks to the server over
//! an in-memory duplex pipe. No process is spawned and no network is used.

use osrs_mcp::mcp::OsrsMcpServer;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::{RoleClient, RunningService},
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

/// Connects a client to a fresh server, completing the MCP handshake.
async fn connect() -> (RunningService<RoleClient, ()>, JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = tokio::spawn(async move {
        let running = OsrsMcpServer::new()
            .serve(server_io)
            .await
            .expect("server should initialise");
        running
            .waiting()
            .await
            .expect("server should shut down cleanly");
    });
    let client = ().serve(client_io).await.expect("client should initialise");
    (client, server)
}

async fn call_server_info(client: &RunningService<RoleClient, ()>) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new("server_info"))
        .await
        .expect("server_info should be callable")
}

#[tokio::test]
async fn initialization_reports_server_identity_and_tools_capability() {
    let (client, _server) = connect().await;

    let info = client.peer_info().expect("server info after handshake");
    let server_info = info.server_info.as_ref().expect("implementation info");
    assert_eq!(server_info.name, "osrs-mcp");
    assert_eq!(server_info.version, env!("CARGO_PKG_VERSION"));
    assert!(info.capabilities.tools.is_some(), "tools capability");
    assert!(info.capabilities.resources.is_none());
    assert!(info.capabilities.prompts.is_none());
}

#[tokio::test]
async fn tool_discovery_lists_only_the_read_only_server_info_tool() {
    let (client, _server) = connect().await;

    let tools = client.list_all_tools().await.expect("tools/list");

    assert_eq!(tools.len(), 1, "tools: {tools:?}");
    let tool = &tools[0];
    assert_eq!(tool.name, "server_info");
    assert!(tool.description.is_some(), "tools need descriptions");
    let annotations = tool.annotations.as_ref().expect("annotations");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert!(tool.output_schema.is_some(), "typed output schema");
}

#[tokio::test]
async fn server_info_returns_version_and_capabilities_as_structured_content() {
    let (client, _server) = connect().await;

    let result = call_server_info(&client).await;

    assert_ne!(result.is_error, Some(true), "result: {result:?}");
    let structured = result.structured_content.expect("structured content");
    assert_eq!(
        structured,
        json!({
            "name": "osrs-mcp",
            "version": env!("CARGO_PKG_VERSION"),
            "read_only": true,
            "capabilities": ["tools"],
        })
    );
}

#[tokio::test]
async fn server_info_text_content_mirrors_structured_content() {
    let (client, _server) = connect().await;

    let result = call_server_info(&client).await;

    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .expect("text block for clients without structured output");
    let parsed: Value = serde_json::from_str(&text.text).expect("JSON text");
    assert_eq!(Some(parsed), result.structured_content);
}

#[tokio::test]
async fn unknown_tool_is_a_protocol_error() {
    let (client, _server) = connect().await;

    let outcome = client
        .call_tool(CallToolRequestParams::new("no_such_tool"))
        .await;

    assert!(outcome.is_err(), "unknown tools must not succeed");
}
