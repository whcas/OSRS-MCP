//! In-process MCP tests for `quest_status`: a real rmcp client talks to the
//! server over an in-memory pipe, reading original synthetic exports from
//! temporary directories. No private export, process or network is involved.

use std::fs;

use osrs_mcp::mcp::OsrsMcpServer;
use osrs_mcp::runelite::{LocalExportSource, QUEST_FILE_NAME};
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

async fn call(client: &Client, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().expect("object arguments");
    client
        .call_tool(CallToolRequestParams::new("quest_status").with_arguments(arguments))
        .await
        .expect("quest_status should be callable")
}

fn source_with(content: &str) -> (TempDir, LocalExportSource) {
    let dir = TempDir::with_prefix("osrs-mcp-quests-").unwrap();
    fs::write(dir.path().join(QUEST_FILE_NAME), content).unwrap();
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

/// The state of synthetic quest `id` in the large export.
const fn state_of(id: u32) -> &'static str {
    if id.is_multiple_of(10) {
        "IN_PROGRESS"
    } else if id.is_multiple_of(3) {
        "NOT_STARTED"
    } else {
        "FINISHED"
    }
}

/// An export of quests 1 to 220 in id order, with `state_of` states. The
/// summary is consistent, so there are no warnings.
fn large_quests_json() -> String {
    let ids: Vec<u32> = (1..=220).collect();
    let count = |state: &str| ids.iter().filter(|id| state_of(**id) == state).count();
    let rows: Vec<String> = ids
        .iter()
        .map(|id| {
            format!(
                r#"{{"id":{id},"name":"Quest {id}","state":"{}"}}"#,
                state_of(*id)
            )
        })
        .collect();
    format!(
        r#"{{"exported_at":"2026-01-01T12:00:00Z","plugin_version":"0.6.0","session_id":"private-session",
           "summary":{{"finished":{},"in_progress":{},"not_started":{}}},"quests":[{}]}}"#,
        count("FINISHED"),
        count("IN_PROGRESS"),
        count("NOT_STARTED"),
        rows.join(",")
    )
}

fn ids(result: &Value) -> Vec<u64> {
    result["quests"]
        .as_array()
        .expect("quests")
        .iter()
        .map(|quest| quest["runelite_id"].as_u64().expect("runelite_id"))
        .collect()
}

#[tokio::test]
async fn quest_status_is_discoverable_read_only_with_a_typed_output() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let tools = client.list_all_tools().await.expect("tools/list");

    let tool = tools
        .iter()
        .find(|tool| tool.name == "quest_status")
        .expect("quest_status is listed");
    let annotations = tool.annotations.as_ref().expect("annotations");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert!(tool.description.is_some(), "needs a description");
    assert!(tool.output_schema.is_some(), "typed output");
    let input = Value::Object((*tool.input_schema).clone());
    let mut properties: Vec<&str> = input["properties"]
        .as_object()
        .expect("input properties")
        .keys()
        .map(String::as_str)
        .collect();
    properties.sort_unstable();
    assert_eq!(
        properties,
        ["limit", "offset", "state"],
        "no path-like input"
    );
}

#[tokio::test]
async fn the_state_filter_is_applied_before_paging() {
    let (_dir, source) = source_with(&large_quests_json());
    let (client, _server) = connect(source).await;
    let finished: Vec<u64> = (1..=220u32)
        .filter(|id| state_of(*id) == "FINISHED")
        .map(u64::from)
        .collect();

    let first = call(&client, json!({ "state": "finished", "limit": 50 })).await;
    let second = call(
        &client,
        json!({ "state": "finished", "limit": 50, "offset": 50 }),
    )
    .await;

    let first = first.structured_content.expect("structured content");
    let second = second.structured_content.expect("structured content");
    // Every row on both pages is finished, and the pages continue the filtered list.
    assert_eq!(ids(&first), finished[..50]);
    assert_eq!(ids(&second), finished[50..100]);
    assert!(
        first["quests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|quest| quest["state"] == "finished")
    );
    // `matched` counts the filtered list before paging.
    assert_eq!(first["matched"], finished.len());
    assert_eq!(first["truncated"], true);
    assert_eq!(first["offset"], 0);
    assert_eq!(second["offset"], 50);
}

#[tokio::test]
async fn counts_cover_the_whole_export_whatever_the_filter_or_page() {
    let (_dir, source) = source_with(&large_quests_json());
    let (client, _server) = connect(source).await;
    let count = |state: &str| (1..=220u32).filter(|id| state_of(*id) == state).count();

    let filtered = call(&client, json!({ "state": "in_progress", "limit": 3 })).await;
    let paged = call(&client, json!({ "offset": 200, "limit": 5 })).await;

    for result in [filtered, paged] {
        let result = result.structured_content.expect("structured content");
        assert_eq!(result["counts"]["recorded"], 220);
        assert_eq!(result["counts"]["finished"], count("FINISHED"));
        assert_eq!(result["counts"]["in_progress"], count("IN_PROGRESS"));
        assert_eq!(result["counts"]["not_started"], count("NOT_STARTED"));
        assert_eq!(result["warnings"], json!([]));
    }
}

#[tokio::test]
async fn an_unfiltered_call_lists_every_state_in_file_order_with_runelite_ids() {
    let (_dir, source) = source_with(&large_quests_json());
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(result["status"], "ok");
    assert_eq!(result["matched"], 220);
    assert_eq!(result["limit"], 100);
    assert_eq!(ids(&result), (1..=100).collect::<Vec<u64>>());
    assert_eq!(result["quests"][2]["state"], "not_started");
    assert_eq!(result["quests"][9]["state"], "in_progress");
    assert_eq!(result["exported_at"]["text"], "2026-01-01T12:00:00Z");
    assert_eq!(result["plugin_version"], "0.6.0");
    assert!(result["read_at"].is_string());
}

#[tokio::test]
async fn a_missing_quest_file_is_file_not_found_never_an_empty_list() {
    let dir = TempDir::with_prefix("osrs-mcp-quests-").unwrap();
    let (client, _server) = connect(LocalExportSource::new(dir.path().to_path_buf())).await;

    let failure = failure_json(&call(&client, json!({})).await);

    assert_eq!(failure["status"], "file_not_found");
    assert!(failure["message"].as_str().unwrap().contains("quests.json"));
}

#[tokio::test]
async fn an_exported_empty_list_is_ok_not_a_failure() {
    let (_dir, source) = source_with(r#"{"exported_at":"2026-01-01T12:00:00Z","quests":[]}"#);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(result["status"], "ok");
    assert_eq!(result["counts"]["recorded"], 0);
    assert_eq!(result["quests"], json!([]));
}

#[tokio::test]
async fn an_unconfigured_server_reports_not_configured() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let failure = failure_json(&call(&client, json!({})).await);

    assert_eq!(failure["status"], "not_configured");
}

#[tokio::test]
async fn a_truncated_file_is_a_retryable_invalid_export_without_file_content() {
    let json = large_quests_json();
    let (_dir, source) = source_with(&json[..json.len() / 2]);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

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
async fn invalid_arguments_are_rejected_before_any_read() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    for arguments in [
        json!({ "limit": 0 }),
        json!({ "limit": 251 }),
        json!({ "state": "done" }),
        json!({ "path": "/x" }),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("quest_status")
                    .with_arguments(arguments.as_object().cloned().unwrap()),
            )
            .await;
        // The unconfigured source would give `not_configured` if a read happened.
        let rejected_before_read = match result {
            Err(_) => true,
            Ok(result) => {
                result.is_error == Some(true)
                    && !result.content[0]
                        .as_text()
                        .is_some_and(|text| text.text.contains("not_configured"))
            }
        };
        assert!(rejected_before_read, "{arguments}");
    }
}

#[tokio::test]
async fn the_response_carries_no_session_identifier_or_path() {
    let (dir, source) = source_with(&large_quests_json());
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

    let text = serde_json::to_string(&result).unwrap();
    let directory = dir.path().to_string_lossy().into_owned();
    for private in ["private-session", "session_id", directory.as_str()] {
        assert!(!text.contains(private), "response contains {private}");
    }
}
