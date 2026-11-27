//! In-process MCP tests for the container tools: a real rmcp client talks to
//! the server over an in-memory pipe, reading original synthetic exports from
//! temporary directories. No private export, process or network is involved.

use std::fs;

use osrs_mcp::mcp::OsrsMcpServer;
use osrs_mcp::runelite::{BANK_FILE_NAME, LocalExportSource};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::{RoleClient, RunningService},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;

type Client = RunningService<RoleClient, ()>;

async fn connect(source: LocalExportSource) -> (Client, JoinHandle<()>) {
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

async fn call(client: &Client, tool: &'static str, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().expect("object arguments");
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("{tool} should be callable: {error}"))
}

/// A bank export of `count` rows, all holding item 1.
fn bank_json(count: usize) -> String {
    let rows: Vec<String> = (0..count)
        .map(|slot| format!(r#"{{"slot":{slot},"id":1,"quantity":{}}}"#, slot + 1))
        .collect();
    format!(
        r#"{{"exported_at":"2026-01-01T12:00:00Z","plugin_version":"0.6.0","session_id":"private-session","kind":"bank","item_count":{count},"items":[{}]}}"#,
        rows.join(",")
    )
}

fn source_with(file: &str, content: &str) -> (TempDir, LocalExportSource) {
    let dir = TempDir::with_prefix("osrs-mcp-containers-").unwrap();
    fs::write(dir.path().join(file), content).unwrap();
    let source = LocalExportSource::new(dir.path().to_path_buf());
    (dir, source)
}

fn failure_json(result: &CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(true), "result: {result:?}");
    assert!(
        result.structured_content.is_none(),
        "failures must not carry structured content"
    );
    let text = &result.content[0].as_text().expect("failure text").text;
    serde_json::from_str(text).expect("failure text is JSON")
}

#[tokio::test]
async fn bank_contents_is_listed_read_only_with_a_typed_output_and_paging_inputs() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let tools = client.list_all_tools().await.expect("tools/list");

    let tool = tools
        .iter()
        .find(|tool| tool.name == "bank_contents")
        .expect("bank_contents is listed");
    let annotations = tool.annotations.as_ref().expect("annotations");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert!(tool.output_schema.is_some(), "typed output");
    let input = Value::Object((*tool.input_schema).clone());
    let mut properties: Vec<&str> = input["properties"]
        .as_object()
        .expect("input properties")
        .keys()
        .map(String::as_str)
        .collect();
    properties.sort_unstable();
    assert_eq!(properties, ["limit", "offset"], "no path-like input");
}

#[tokio::test]
async fn a_synthetic_bank_is_paged_in_file_order() {
    let (_dir, source) = source_with(BANK_FILE_NAME, &bank_json(5));
    let (client, _server) = connect(source).await;

    let first = call(&client, "bank_contents", json!({ "limit": 2 })).await;
    let second = call(&client, "bank_contents", json!({ "offset": 2, "limit": 2 })).await;

    let first = first.structured_content.expect("structured content");
    let second = second.structured_content.expect("structured content");
    assert_eq!(first["status"], "ok");
    assert_eq!(first["container"], "bank");
    assert_eq!(first["total_rows"], 5);
    assert_eq!(first["truncated"], true);
    assert_eq!(first["rows"][0]["slot"], 0);
    assert_eq!(first["rows"][1]["quantity"], 2);
    assert_eq!(second["rows"][0]["slot"], 2);
    assert_eq!(second["offset"], 2);
    assert_eq!(first["exported_at"]["text"], "2026-01-01T12:00:00Z");
    assert_eq!(first["plugin_version"], "0.6.0");
    assert!(first["read_at"].is_string());
}

#[tokio::test]
async fn an_empty_bank_is_ok_and_a_missing_file_is_a_failure() {
    let (_dir, source) = source_with(BANK_FILE_NAME, &bank_json(0));
    let (client, _server) = connect(source).await;
    let empty = call(&client, "bank_contents", json!({})).await;

    let empty = empty.structured_content.expect("structured content");
    assert_eq!(
        (empty["status"].as_str(), empty["total_rows"].as_u64()),
        (Some("ok"), Some(0))
    );

    let dir = TempDir::with_prefix("osrs-mcp-containers-").unwrap();
    let (client, _server) = connect(LocalExportSource::new(dir.path().to_path_buf())).await;
    let missing = failure_json(&call(&client, "bank_contents", json!({})).await);
    assert_eq!(missing["status"], "file_not_found");
    assert!(missing["message"].as_str().unwrap().contains("bank.json"));
}

#[tokio::test]
async fn an_unconfigured_server_reports_not_configured() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let failure = failure_json(&call(&client, "bank_contents", json!({})).await);

    assert_eq!(failure["status"], "not_configured");
}

#[tokio::test]
async fn a_truncated_file_is_a_retryable_invalid_export_without_file_content() {
    let json = bank_json(3);
    let (_dir, source) = source_with(BANK_FILE_NAME, &json[..json.len() / 2]);
    let (client, _server) = connect(source).await;

    let result = call(&client, "bank_contents", json!({})).await;

    let failure = failure_json(&result);
    assert_eq!(failure["status"], "invalid_export");
    assert_eq!(failure["retryable"], true);
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("exported_at")
    );
}

#[tokio::test]
async fn out_of_range_paging_is_rejected_before_any_read() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    for arguments in [
        json!({ "limit": 0 }),
        json!({ "limit": 201 }),
        json!({ "path": "/x" }),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("bank_contents")
                    .with_arguments(arguments.as_object().cloned().unwrap()),
            )
            .await;
        // An invalid argument is a protocol error or an error result, never data.
        let rejected = result.map_or(true, |result| result.is_error == Some(true));
        assert!(rejected, "{arguments} should be rejected");
    }
}

#[tokio::test]
async fn the_response_carries_no_session_identifier_or_path() {
    let (dir, source) = source_with(BANK_FILE_NAME, &bank_json(2));
    let (client, _server) = connect(source).await;

    let result = call(&client, "bank_contents", json!({})).await;

    let text = serde_json::to_string(&result).unwrap();
    let directory = dir.path().to_string_lossy().into_owned();
    for private in ["private-session", "session_id", directory.as_str()] {
        assert!(!text.contains(private), "response contains {private}");
    }
}
