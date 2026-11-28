//! In-process MCP tests for the container tools: a real rmcp client talks to
//! the server over an in-memory pipe, reading original synthetic exports from
//! temporary directories. No private export, process or network is involved.

mod common;

use common::{Client, connect, failure_json, source_in};
use osrs_mcp::runelite::{BANK_FILE_NAME, EQUIPMENT_FILE_NAME, LocalExportSource};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use tempfile::TempDir;

async fn call(client: &Client, tool: &'static str, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().expect("object arguments");
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("{tool} should be callable: {error}"))
}

fn source_with(file: &str, content: &str) -> (TempDir, LocalExportSource) {
    source_in("osrs-mcp-containers-", file, content)
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
    assert_eq!(
        properties,
        ["item_ids", "limit", "offset"],
        "no path-like input"
    );
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

/// A bank of 1,200 rows. Slot `n` holds item `10_000 + n`, except that item 555
/// is also in slots 3, 700 and 1,150 and item 777 is in slot 1,190.
fn large_bank_json() -> String {
    let rows: Vec<String> = (0..1_200)
        .map(|slot| {
            let id = match slot {
                3 | 700 | 1_150 => 555,
                1_190 => 777,
                _ => 10_000 + slot,
            };
            format!(r#"{{"slot":{slot},"id":{id},"quantity":1}}"#)
        })
        .collect();
    format!(
        r#"{{"exported_at":"2026-01-01T12:00:00Z","kind":"bank","item_count":1200,"items":[{}]}}"#,
        rows.join(",")
    )
}

fn slots(result: &Value) -> Vec<u64> {
    result["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["slot"].as_u64().expect("slot"))
        .collect()
}

#[tokio::test]
async fn an_item_filter_searches_the_whole_bank_before_paging() {
    let (_dir, source) = source_with(BANK_FILE_NAME, &large_bank_json());
    let (client, _server) = connect(source).await;
    // 555 repeats in different slots and 777 is near the end; 888 is not in the bank.
    let filter = json!([555, 777, 888]);

    let first = call(
        &client,
        "bank_contents",
        json!({ "item_ids": filter, "limit": 2 }),
    )
    .await;
    let second = call(
        &client,
        "bank_contents",
        json!({ "item_ids": filter, "limit": 2, "offset": 2 }),
    )
    .await;

    let first = first.structured_content.expect("structured content");
    let second = second.structured_content.expect("structured content");
    // Matches are counted over the whole bank, before paging.
    assert_eq!(first["snapshot_rows"], 1_200);
    assert_eq!(first["total_rows"], 4);
    assert_eq!(slots(&first), [3, 700]);
    assert_eq!(first["truncated"], true);
    // 777 is on a later page, so it is not reported absent; only 888 is.
    assert_eq!(
        first["item_filter"]["requested_item_ids"],
        json!([555, 777, 888])
    );
    assert_eq!(first["item_filter"]["unmatched_item_ids"], json!([888]));
    assert_eq!(second["item_filter"]["unmatched_item_ids"], json!([888]));
    // The later page holds the third repeat and the match near the end, in file order.
    assert_eq!(slots(&second), [1_150, 1_190]);
    assert_eq!(second["rows"][0]["item_id"], 555);
    assert_eq!(second["rows"][1]["item_id"], 777);
    assert_eq!(second["truncated"], false);
}

#[tokio::test]
async fn an_unfiltered_call_on_a_large_bank_reports_no_filter_section() {
    let (_dir, source) = source_with(BANK_FILE_NAME, &large_bank_json());
    let (client, _server) = connect(source).await;

    let result = call(&client, "bank_contents", json!({ "limit": 5 })).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(result["total_rows"], 1_200);
    assert_eq!(result["snapshot_rows"], 1_200);
    assert_eq!(result["item_filter"], Value::Null);
}

#[tokio::test]
async fn a_filter_on_an_empty_bank_is_ok_with_every_id_unmatched() {
    let (_dir, source) = source_with(BANK_FILE_NAME, &bank_json(0));
    let (client, _server) = connect(source).await;

    let result = call(&client, "bank_contents", json!({ "item_ids": [1, 2] })).await;

    let result = result.structured_content.expect("structured content");
    assert_eq!(result["status"], "ok");
    assert_eq!(
        (
            result["total_rows"].as_u64(),
            result["snapshot_rows"].as_u64()
        ),
        (Some(0), Some(0))
    );
    assert_eq!(result["item_filter"]["unmatched_item_ids"], json!([1, 2]));
}

#[tokio::test]
async fn a_filter_on_a_missing_file_is_still_file_not_found() {
    let dir = TempDir::with_prefix("osrs-mcp-containers-").unwrap();
    let (client, _server) = connect(LocalExportSource::new(dir.path().to_path_buf())).await;

    let failure = failure_json(&call(&client, "bank_contents", json!({ "item_ids": [1] })).await);

    assert_eq!(failure["status"], "file_not_found");
}

#[tokio::test]
async fn empty_or_oversized_item_id_lists_are_rejected_before_any_read() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;
    let too_many: Vec<u32> = (1..=51).collect();

    for arguments in [json!({ "item_ids": [] }), json!({ "item_ids": too_many })] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("bank_contents")
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

/// An equipment export with the given `(slot, id)` rows, one of each.
fn equipment_json(rows: &[(u16, u32)]) -> String {
    let items: Vec<String> = rows
        .iter()
        .map(|(slot, id)| format!(r#"{{"slot":{slot},"id":{id},"quantity":1}}"#))
        .collect();
    format!(
        r#"{{"exported_at":"2026-01-01T12:00:00Z","kind":"equipment","item_count":{},"items":[{}]}}"#,
        rows.len(),
        items.join(",")
    )
}

#[tokio::test]
async fn equipment_contents_is_listed_read_only_with_no_inputs() {
    let (client, _server) = connect(LocalExportSource::unconfigured()).await;

    let tools = client.list_all_tools().await.expect("tools/list");

    let tool = tools
        .iter()
        .find(|tool| tool.name == "equipment_contents")
        .expect("equipment_contents is listed");
    let annotations = tool.annotations.as_ref().expect("annotations");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert!(tool.output_schema.is_some(), "typed output");
    let input = Value::Object((*tool.input_schema).clone());
    assert!(
        input
            .get("properties")
            .is_none_or(|p| p.as_object().is_some_and(serde_json::Map::is_empty)),
        "no inputs: {input}"
    );
}

#[tokio::test]
async fn populated_equipment_is_returned_in_file_order() {
    let (_dir, source) = source_with(
        EQUIPMENT_FILE_NAME,
        &equipment_json(&[(3, 4151), (0, 1163)]),
    );
    let (client, _server) = connect(source).await;

    let result = call(&client, "equipment_contents", json!({})).await;

    let equipment = result.structured_content.expect("structured content");
    assert_eq!(equipment["status"], "ok");
    assert_eq!(equipment["container"], "equipment");
    assert_eq!(equipment["total_rows"], 2);
    assert_eq!(equipment["rows"][0]["slot"], 3);
    assert_eq!(equipment["rows"][0]["item_id"], 4151);
    assert_eq!(equipment["rows"][1]["slot"], 0);
    assert_eq!(equipment["warnings"], json!([]));
}

#[tokio::test]
async fn empty_equipment_is_ok_and_a_missing_file_is_a_failure() {
    let (_dir, source) = source_with(EQUIPMENT_FILE_NAME, &equipment_json(&[]));
    let (client, _server) = connect(source).await;
    let empty = call(&client, "equipment_contents", json!({})).await;
    let empty = empty.structured_content.expect("structured content");
    assert_eq!(
        (empty["status"].as_str(), empty["total_rows"].as_u64()),
        (Some("ok"), Some(0))
    );

    // A bank file in the directory is never read as equipment.
    let (_dir, source) = source_with(BANK_FILE_NAME, &bank_json(1));
    let (client, _server) = connect(source).await;
    let missing = failure_json(&call(&client, "equipment_contents", json!({})).await);
    assert_eq!(missing["status"], "file_not_found");
    assert!(
        missing["message"]
            .as_str()
            .unwrap()
            .contains("equipment.json")
    );
}

#[tokio::test]
async fn a_file_holding_another_container_kind_is_an_invalid_export() {
    let (_dir, source) = source_with(EQUIPMENT_FILE_NAME, &bank_json(1));
    let (client, _server) = connect(source).await;

    let failure = failure_json(&call(&client, "equipment_contents", json!({})).await);

    assert_eq!(failure["status"], "invalid_export");
    assert_eq!(failure["retryable"], false);
}
