//! In-process MCP tests for `diary_status`: a real rmcp client talks to the
//! server over an in-memory pipe, reading original synthetic exports from
//! temporary directories. No private export, process or network is involved.

use std::fs;

use osrs_mcp::mcp::OsrsMcpServer;
use osrs_mcp::runelite::{DIARY_FILE_NAME, LocalExportSource};
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
        .call_tool(CallToolRequestParams::new("diary_status").with_arguments(arguments))
        .await
        .expect("diary_status should be callable")
}

fn source_with(content: &str) -> (TempDir, LocalExportSource) {
    let dir = TempDir::with_prefix("osrs-mcp-diaries-").unwrap();
    fs::write(dir.path().join(DIARY_FILE_NAME), content).unwrap();
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

/// Alpha's medium tier contradicts itself (flagged complete, count 5, two
/// listed tasks of which one is done); Beta has an easy tier with no task list;
/// Gamma has a hard tier and an unrecognised tier.
const DIARIES: &str = r#"{
  "exported_at": "2026-01-01T12:00:00Z",
  "plugin_version": "0.6.0",
  "session_id": "private-session",
  "diaries": {
    "Alpha": {
      "easy": {"complete": true, "tasks_done": 2, "tasks": [
        {"name": "Task A1", "complete": true}, {"name": "Task A2", "complete": true}]},
      "medium": {"complete": true, "tasks_done": 5, "tasks": [
        {"name": "Task M1", "complete": true}, {"name": "Task M2", "complete": false}]}
    },
    "Beta": { "easy": {"complete": false, "tasks_done": 0} },
    "Gamma": { "hard": {"complete": false, "tasks_done": 1}, "mystery": {"complete": true, "tasks_done": 0} }
  }
}"#;

fn area_names(result: &Value) -> Vec<&str> {
    result["areas"]
        .as_array()
        .expect("areas")
        .iter()
        .map(|area| area["name"].as_str().expect("name"))
        .collect()
}

#[tokio::test]
async fn diary_status_is_discoverable_read_only_with_a_typed_output() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let tools = client.list_all_tools().await.expect("tools/list");

    let tool = tools
        .iter()
        .find(|tool| tool.name == "diary_status")
        .expect("diary_status is listed");
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
        ["area", "include_tasks", "limit", "offset", "tier"],
        "no path-like input"
    );
}

#[tokio::test]
async fn conflicting_observations_are_reported_side_by_side_with_named_warnings() {
    let (_dir, source) = source_with(DIARIES);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(result["status"], "ok");
    assert_eq!(area_names(&result), ["Alpha", "Beta", "Gamma"]);
    let medium = &result["areas"][0]["tiers"][1];
    assert_eq!(medium["tier"], "medium");
    assert_eq!(medium["flagged_complete"], true);
    assert_eq!(medium["tasks_done"], 5);
    assert_eq!(medium["tasks_listed"], 2);
    assert_eq!(medium["tasks_listed_complete"], 1);
    assert_eq!(
        medium["tasks"],
        Value::Null,
        "task names need include_tasks"
    );
    let beta_easy = &result["areas"][1]["tiers"][0];
    assert_eq!(beta_easy["tasks_listed"], Value::Null);
    let warnings = &result["warnings"];
    assert!(warnings.as_array().unwrap().contains(&json!(
        { "kind": "complete_with_incomplete_task", "area": "Alpha", "tier": "medium" }
    )));
    assert!(warnings.as_array().unwrap().contains(&json!(
        { "kind": "task_count_mismatch", "area": "Alpha", "tier": "medium" }
    )));
    assert!(warnings.as_array().unwrap().contains(&json!(
        { "kind": "unknown_tier", "area": "Gamma" }
    )));
    assert_eq!(result["exported_at"]["text"], "2026-01-01T12:00:00Z");
}

#[tokio::test]
async fn task_names_are_returned_only_for_a_named_area_with_include_tasks() {
    let (_dir, source) = source_with(DIARIES);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({ "area": "alpha", "include_tasks": true })).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(area_names(&result), ["Alpha"]);
    let tasks = &result["areas"][0]["tiers"][1]["tasks"];
    assert_eq!(
        tasks[0],
        json!({ "name": "Task M1", "name_truncated": false, "complete": true })
    );
    assert_eq!(tasks[1]["complete"], false);
    // Beta's tier has no list, so there is nothing to return even if asked.
    let beta = call(&client, json!({ "area": "Beta", "include_tasks": true })).await;
    let beta = beta.structured_content.expect("structured content");
    assert_eq!(beta["areas"][0]["tiers"][0]["tasks"], Value::Null);
}

#[tokio::test]
async fn filters_apply_before_paging_and_counts_cover_the_whole_export() {
    let (_dir, source) = source_with(DIARIES);
    let (client, _server) = connect(source).await;

    let first = call(&client, json!({ "tier": "easy", "limit": 1 })).await;
    let second = call(&client, json!({ "tier": "easy", "limit": 1, "offset": 1 })).await;

    let first = first.structured_content.expect("structured content");
    let second = second.structured_content.expect("structured content");
    // Only Alpha and Beta have an easy tier; Gamma is dropped before paging.
    assert_eq!(
        (
            area_names(&first),
            first["matched"].as_u64(),
            first["truncated"].as_bool()
        ),
        (vec!["Alpha"], Some(2), Some(true))
    );
    assert_eq!(
        (
            area_names(&second),
            second["matched"].as_u64(),
            second["truncated"].as_bool()
        ),
        (vec!["Beta"], Some(2), Some(false))
    );
    assert_eq!(first["areas"][0]["tiers"].as_array().map(Vec::len), Some(1));
    for result in [&first, &second] {
        assert_eq!(result["counts"]["areas_recorded"], 3);
        assert_eq!(result["counts"]["tiers_recorded"], 4);
        assert_eq!(result["counts"]["tiers_flagged_complete"], 2);
    }
}

#[tokio::test]
async fn an_unrecorded_area_is_an_empty_ok_result_not_an_incomplete_one() {
    let (_dir, source) = source_with(DIARIES);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({ "area": "Nowhere" })).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(result["status"], "ok");
    assert_eq!(result["areas"], json!([]));
    assert_eq!(result["matched"], 0);
}

#[tokio::test]
async fn a_missing_diary_file_is_file_not_found_and_an_empty_export_is_ok() {
    let dir = TempDir::with_prefix("osrs-mcp-diaries-").unwrap();
    let (client, _server) = connect(LocalExportSource::new(dir.path().to_path_buf())).await;
    let failure = failure_json(&call(&client, json!({})).await);
    assert_eq!(failure["status"], "file_not_found");
    assert!(
        failure["message"]
            .as_str()
            .unwrap()
            .contains("diaries.json")
    );

    let (_dir, source) = source_with(r#"{"exported_at":"2026-01-01T12:00:00Z","diaries":{}}"#);
    let (client, _server) = connect(source).await;
    let empty = call(&client, json!({})).await;
    let empty = empty.structured_content.expect("structured content");
    assert_eq!(empty["status"], "ok");
    assert_eq!(empty["counts"]["areas_recorded"], 0);
}

#[tokio::test]
async fn an_unconfigured_server_reports_not_configured() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let failure = failure_json(&call(&client, json!({})).await);

    assert_eq!(failure["status"], "not_configured");
}

#[tokio::test]
async fn a_truncated_file_is_a_retryable_invalid_export_without_file_content() {
    let (_dir, source) = source_with(&DIARIES[..DIARIES.len() / 2]);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

    let failure = failure_json(&result);
    assert_eq!(failure["status"], "invalid_export");
    assert_eq!(failure["retryable"], true);
    assert!(!serde_json::to_string(&result).unwrap().contains("Alpha"));
}

#[tokio::test]
async fn invalid_arguments_are_rejected_before_any_read() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    for arguments in [
        json!({ "include_tasks": true }),
        json!({ "limit": 0 }),
        json!({ "limit": 65 }),
        json!({ "tier": "legendary" }),
        json!({ "area": "" }),
        json!({ "path": "/x" }),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("diary_status")
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
    let (dir, source) = source_with(DIARIES);
    let (client, _server) = connect(source).await;

    let result = call(&client, json!({})).await;

    let text = serde_json::to_string(&result).unwrap();
    let directory = dir.path().to_string_lossy().into_owned();
    for private in ["private-session", "session_id", directory.as_str()] {
        assert!(!text.contains(private), "response contains {private}");
    }
}
