//! In-process MCP tests for `character_summary`: a real rmcp client talks to
//! the server over an in-memory pipe, reading original synthetic exports from
//! temporary directories. No private export, process or network is involved.

mod common;

use common::{Client, connect, failure_json, source_in};
use osrs_mcp::runelite::{CHARACTER_FILE_NAME, LocalExportSource};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use tempfile::TempDir;

const SYNTHETIC_EXPORT: &[u8] = include_bytes!("fixtures/runelite_export/character.synthetic.json");

/// A directory holding `content` as `character.json`, with a source reading it.
fn source_with(content: &[u8]) -> (TempDir, LocalExportSource) {
    source_in("osrs-mcp-summary-", CHARACTER_FILE_NAME, content)
}

async fn call(client: &Client) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new("character_summary"))
        .await
        .expect("character_summary should be callable")
}

#[tokio::test]
async fn a_configured_synthetic_export_is_summarised() {
    let (_dir, source) = source_with(SYNTHETIC_EXPORT);
    let (client, _server) = connect(source).await;

    let result = call(&client).await;

    assert_ne!(result.is_error, Some(true), "result: {result:?}");
    let summary = result.structured_content.expect("structured content");
    assert_eq!(summary["status"], "ok");
    assert_eq!(
        summary["exported_at"]["text"],
        "2026-01-01T12:00:00.123456Z"
    );
    assert_eq!(summary["plugin_version"], "0.6.0");
    assert_eq!(summary["skill_count"], 24);
    assert_eq!(summary["skills_truncated"], false);
    assert_eq!(summary["skills"].as_array().map(Vec::len), Some(24));
    assert!(summary["read_at"].is_string());
    // The synthetic export is long past, so it has a positive age.
    assert!(summary["seconds_since_last_change"].as_u64().unwrap() > 0);
    assert_eq!(summary["exported_at_in_future"], false);
}

#[tokio::test]
async fn the_summary_carries_no_account_identifiers() {
    let (_dir, source) = source_with(SYNTHETIC_EXPORT);
    let (client, _server) = connect(source).await;

    let result = call(&client).await;

    let text = serde_json::to_string(&result).unwrap();
    for private in [
        "Synthetic Account",
        "synthetic-session",
        "world_types",
        "account",
    ] {
        assert!(!text.contains(private), "response contains {private}");
    }
}

#[tokio::test]
async fn a_future_export_time_is_reported_without_an_age() {
    let (_dir, source) = source_with(
        br#"{"exported_at":"2999-01-01T00:00:00Z","stats":{"Attack":{"real_level":1,"experience":0}}}"#,
    );
    let (client, _server) = connect(source).await;

    let result = call(&client).await;

    let summary = result.structured_content.expect("structured content");
    assert_eq!(summary["exported_at_in_future"], true);
    assert!(summary["seconds_since_last_change"].is_null());
}

#[tokio::test]
async fn an_unconfigured_server_reports_not_configured_and_still_serves_other_tools() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let failure = failure_json(&call(&client).await);
    let info = client
        .call_tool(CallToolRequestParams::new("server_info"))
        .await
        .expect("server_info");

    assert_eq!(failure["status"], "not_configured");
    assert_ne!(info.is_error, Some(true));
}

#[tokio::test]
async fn a_directory_without_the_file_reports_file_not_found() {
    let dir = TempDir::with_prefix("osrs-mcp-summary-").unwrap();
    let (client, _server) = connect(LocalExportSource::new(dir.path().to_path_buf())).await;

    let failure = failure_json(&call(&client).await);

    assert_eq!(failure["status"], "file_not_found");
}

#[tokio::test]
async fn a_missing_directory_reports_directory_unavailable() {
    let dir = TempDir::with_prefix("osrs-mcp-summary-").unwrap();
    let (client, _server) = connect(LocalExportSource::new(dir.path().join("absent"))).await;

    let failure = failure_json(&call(&client).await);

    assert_eq!(failure["status"], "directory_unavailable");
}

#[tokio::test]
async fn a_truncated_file_is_invalid_and_retryable() {
    let (_dir, source) = source_with(&SYNTHETIC_EXPORT[..SYNTHETIC_EXPORT.len() / 2]);
    let (client, _server) = connect(source).await;

    let failure = failure_json(&call(&client).await);

    assert_eq!(failure["status"], "invalid_export");
    assert_eq!(failure["retryable"], true);
}

#[tokio::test]
async fn invalid_json_is_invalid_and_not_retryable() {
    let (_dir, source) = source_with(b"not json");
    let (client, _server) = connect(source).await;

    let failure = failure_json(&call(&client).await);

    assert_eq!(failure["status"], "invalid_export");
    assert_eq!(failure["retryable"], false);
}

#[tokio::test]
async fn an_oversized_file_is_unreadable_with_its_reason() {
    let (_dir, source) = source_with(&vec![b' '; 256 * 1024 + 1]);
    let (client, _server) = connect(source).await;

    let failure = failure_json(&call(&client).await);

    assert_eq!(failure["status"], "unreadable");
    assert_eq!(failure["reason"], "too_large");
}

#[tokio::test]
async fn failures_never_contain_the_configured_path() {
    let dir = TempDir::with_prefix("osrs-mcp-summary-").unwrap();
    let directory_name = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let (client, _server) = connect(LocalExportSource::new(dir.path().to_path_buf())).await;

    let result = call(&client).await;

    let text = serde_json::to_string(&result).unwrap();
    assert!(!text.contains(&directory_name), "{text}");
}
