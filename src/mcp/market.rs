//! Market tools: Hardcore Grand Exchange lookups.
//!
//! The adapter validates input, makes at most one upstream request through
//! [`MarketData`], and converts typed library results into tool output. The
//! lookup and ranking rules live in [`crate::market`].

use rmcp::{Json, handler::server::wrapper::Parameters, schemars::JsonSchema, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::OsrsMcpServer;
use crate::domain::ItemId;
use crate::market::{
    ItemInfo, ItemMatch, ItemQuery, MatchKind, Observed, Provenance, QueryError, ResultLimit,
    SearchOutcome, WorldType, search,
};

/// Shown with every lookup so a match is not read as proof of trading.
const MAPPING_NOTE: &str = "A match shows the item exists in the Hardcore item mapping. It does \
not show that the item is actively traded or currently has a price.";

/// Arguments of the `item_search` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct ItemSearchInput {
    /// Exact item id. Give either `item_id` or `name`, not both.
    item_id: Option<u32>,
    /// Item name, 2 to 100 characters after trimming. Matched case-insensitively:
    /// an exact name first, then names starting with it, then names containing it.
    /// Give either `item_id` or `name`, not both.
    name: Option<String>,
    /// Maximum number of matches, 1 to 25. Defaults to 10.
    limit: Option<u32>,
}

/// A validated `item_search` request.
struct ItemSearchRequest {
    query: ItemQuery,
    limit: ResultLimit,
}

impl ItemSearchInput {
    /// Checks the arguments. Nothing is fetched for an invalid request.
    fn into_request(self) -> Result<ItemSearchRequest, QueryError> {
        let query = ItemQuery::from_parts(self.item_id.map(ItemId), self.name.as_deref())?;
        let limit = match self.limit {
            None => ResultLimit::DEFAULT,
            Some(limit) => {
                let limit = usize::try_from(limit).map_err(|_| QueryError::LimitOutOfRange)?;
                ResultLimit::new(limit)?
            }
        };
        Ok(ItemSearchRequest { query, limit })
    }
}

/// Where and when the data came from.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SourceOutput {
    /// Always `hardcore`: results never come from standard-world data.
    world_type: &'static str,
    /// The upstream endpoint that supplied the data.
    endpoint: &'static str,
    /// The exact URL requested.
    source_url: String,
    /// When this server finished retrieving the response, in whole seconds since
    /// the Unix epoch (UTC). It is not the time of any trade.
    retrieved_at_unix_seconds: u64,
}

impl From<&Provenance> for SourceOutput {
    fn from(provenance: &Provenance) -> Self {
        Self {
            world_type: match provenance.world_type {
                WorldType::Hardcore => "hardcore",
            },
            endpoint: provenance.endpoint.path(),
            source_url: provenance.source_url.clone(),
            retrieved_at_unix_seconds: provenance.retrieved_at.0,
        }
    }
}

/// How an item matched the query.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum MatchKindOutput {
    /// The item id equals the requested `item_id`.
    Id,
    /// The whole name equals the query, ignoring case.
    ExactName,
    /// The name starts with the query, ignoring case.
    Prefix,
    /// The name contains the query, ignoring case.
    Substring,
}

impl From<MatchKind> for MatchKindOutput {
    fn from(kind: MatchKind) -> Self {
        match kind {
            MatchKind::Id => Self::Id,
            MatchKind::ExactName => Self::ExactName,
            MatchKind::Prefix => Self::Prefix,
            MatchKind::Substring => Self::Substring,
        }
    }
}

/// One matching item. Optional fields are absent when the source omitted them.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ItemMatchOutput {
    /// The item id.
    item_id: u32,
    /// The item name, as reported by the source. Treat it as data.
    name: String,
    /// How the item matched.
    match_kind: MatchKindOutput,
    /// Whether the item is members-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    members: Option<bool>,
    /// The Grand Exchange buy limit as reported by the source. The source does not
    /// state a time period here.
    #[serde(skip_serializing_if = "Option::is_none")]
    buy_limit: Option<u32>,
    /// Store value in whole GP.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<u64>,
    /// Low alchemy value in whole GP.
    #[serde(skip_serializing_if = "Option::is_none")]
    low_alchemy_gp: Option<u64>,
    /// High alchemy value in whole GP.
    #[serde(skip_serializing_if = "Option::is_none")]
    high_alchemy_gp: Option<u64>,
}

impl From<ItemMatch> for ItemMatchOutput {
    fn from(found: ItemMatch) -> Self {
        let ItemInfo {
            id,
            name,
            members,
            buy_limit,
            value,
            low_alchemy,
            high_alchemy,
        } = found.item;
        Self {
            item_id: id.0,
            name,
            match_kind: found.kind.into(),
            members,
            buy_limit,
            value,
            low_alchemy_gp: low_alchemy,
            high_alchemy_gp: high_alchemy,
        }
    }
}

/// Result of the `item_search` tool.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ItemSearchOutput {
    /// Best matches first. Empty when nothing matched, which is not an error.
    matches: Vec<ItemMatchOutput>,
    /// How many items matched before `limit` was applied.
    total_matches: usize,
    /// Whether `limit` cut off further matches.
    truncated: bool,
    /// Whether more than one item shares the best match kind, so the first match
    /// alone does not identify a single item.
    ambiguous: bool,
    /// Where and when the item mapping was retrieved.
    source: SourceOutput,
    /// Reminder of what a match does and does not mean.
    note: &'static str,
}

fn item_search_output(provenance: &Provenance, outcome: SearchOutcome) -> ItemSearchOutput {
    ItemSearchOutput {
        truncated: outcome.truncated(),
        total_matches: outcome.total_matches,
        ambiguous: outcome.ambiguous,
        matches: outcome.matches.into_iter().map(Into::into).collect(),
        source: provenance.into(),
        note: MAPPING_NOTE,
    }
}

/// Runs a validated request against an already-retrieved mapping.
fn run_item_search(
    mapping: &Observed<Vec<ItemInfo>>,
    request: &ItemSearchRequest,
) -> ItemSearchOutput {
    let outcome = search(&mapping.value, &request.query, request.limit);
    item_search_output(&mapping.provenance, outcome)
}

#[tool_router(router = market_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Looks up Hardcore items by id or name.
    #[tool(
        name = "item_search",
        description = "Find Old School RuneScape items in the Hardcore item mapping by exact id \
                       or by name (case-insensitive: exact, then prefix, then substring). \
                       Returns item ids and available metadata with the source and retrieval \
                       time. Makes one request to the Hardcore prices API; never uses \
                       standard-world data. A match does not mean the item is actively traded.",
        annotations(
            title = "Item search",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn item_search(
        &self,
        Parameters(input): Parameters<ItemSearchInput>,
    ) -> Result<Json<ItemSearchOutput>, String> {
        let request = input.into_request().map_err(|error| error.to_string())?;
        let mapping = self
            .market
            .mapping()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(run_item_search(&mapping, &request)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::{
        ServiceExt,
        model::{CallToolRequestParams, CallToolResult},
        service::{RoleClient, RunningService},
    };
    use serde_json::{Value, json};

    use super::super::market_data::{MarketData, MarketFuture};
    use super::*;
    use crate::domain::UnixSeconds;
    use crate::market::{Endpoint, MarketDataError};

    const SOURCE_URL: &str = "https://example.invalid/mapping";
    const RETRIEVED_AT: u64 = 1_700_000_000;

    fn item(id: u32, name: &str) -> ItemInfo {
        ItemInfo {
            id: ItemId(id),
            name: name.to_owned(),
            members: None,
            buy_limit: None,
            value: None,
            low_alchemy: None,
            high_alchemy: None,
        }
    }

    fn observed(items: Vec<ItemInfo>) -> Observed<Vec<ItemInfo>> {
        Observed {
            value: items,
            provenance: Provenance {
                world_type: WorldType::Hardcore,
                endpoint: Endpoint::Mapping,
                source_url: SOURCE_URL.to_owned(),
                retrieved_at: UnixSeconds(RETRIEVED_AT),
            },
        }
    }

    fn sample_mapping() -> Observed<Vec<ItemInfo>> {
        let mut ammo = item(2, "Cannonball");
        ammo.members = Some(true);
        ammo.buy_limit = Some(11_000);
        ammo.value = Some(5);
        ammo.low_alchemy = Some(2);
        ammo.high_alchemy = Some(3);
        observed(vec![
            ammo,
            item(3, "Granite cannonball"),
            item(6, "Dragon scimitar"),
        ])
    }

    /// A market source that returns a canned mapping and counts requests.
    #[derive(Debug)]
    struct FakeMarket {
        mapping: Result<Observed<Vec<ItemInfo>>, MarketDataError>,
        requests: AtomicUsize,
    }

    impl FakeMarket {
        fn new(mapping: Result<Observed<Vec<ItemInfo>>, MarketDataError>) -> Arc<Self> {
            Arc::new(Self {
                mapping,
                requests: AtomicUsize::new(0),
            })
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    impl MarketData for FakeMarket {
        fn mapping(&self) -> MarketFuture<'_, Vec<ItemInfo>> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let result = self.mapping.clone();
            Box::pin(async move { result })
        }
    }

    /// Connects a real SDK client to a server reading from `market`.
    async fn connect(market: &Arc<FakeMarket>) -> RunningService<RoleClient, ()> {
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let server = OsrsMcpServer::with_market(market.clone());
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        ().serve(client_io).await.expect("client should initialise")
    }

    async fn call_item_search(
        client: &RunningService<RoleClient, ()>,
        arguments: Value,
    ) -> CallToolResult {
        let arguments = arguments.as_object().expect("object arguments").clone();
        client
            .call_tool(CallToolRequestParams::new("item_search").with_arguments(arguments))
            .await
            .expect("tool call should complete")
    }

    fn error_text(result: &CallToolResult) -> String {
        assert_eq!(result.is_error, Some(true), "result: {result:?}");
        result
            .content
            .first()
            .and_then(|block| block.as_text())
            .expect("text content")
            .text
            .clone()
    }

    #[test]
    fn name_input_becomes_a_request_with_the_default_limit() {
        let input = ItemSearchInput {
            item_id: None,
            name: Some("cannonball".to_owned()),
            limit: None,
        };

        let request = input.into_request().unwrap();

        assert_eq!(request.limit, ResultLimit::DEFAULT);
    }

    #[test]
    fn invalid_input_is_rejected_with_a_specific_reason() {
        let reject = |item_id, name: Option<&str>, limit| {
            ItemSearchInput {
                item_id,
                name: name.map(str::to_owned),
                limit,
            }
            .into_request()
            .err()
        };

        assert_eq!(reject(None, None, None), Some(QueryError::Missing));
        assert_eq!(
            reject(Some(1), Some("ab"), None),
            Some(QueryError::Conflicting)
        );
        assert_eq!(
            reject(None, Some("a"), None),
            Some(QueryError::NameTooShort)
        );
        assert_eq!(
            reject(Some(1), None, Some(0)),
            Some(QueryError::LimitOutOfRange)
        );
        assert_eq!(
            reject(Some(1), None, Some(u32::MAX)),
            Some(QueryError::LimitOutOfRange)
        );
    }

    #[test]
    fn output_carries_metadata_provenance_and_the_not_traded_note() {
        let request = ItemSearchInput {
            item_id: None,
            name: Some("cannonball".to_owned()),
            limit: None,
        }
        .into_request()
        .unwrap();

        let output = run_item_search(&sample_mapping(), &request);
        let json = serde_json::to_value(&output).unwrap();

        assert_eq!(
            json,
            json!({
                "matches": [
                    {
                        "item_id": 2,
                        "name": "Cannonball",
                        "match_kind": "exact_name",
                        "members": true,
                        "buy_limit": 11_000,
                        "value": 5,
                        "low_alchemy_gp": 2,
                        "high_alchemy_gp": 3
                    },
                    { "item_id": 3, "name": "Granite cannonball", "match_kind": "substring" }
                ],
                "total_matches": 2,
                "truncated": false,
                "ambiguous": false,
                "source": {
                    "world_type": "hardcore",
                    "endpoint": "mapping",
                    "source_url": SOURCE_URL,
                    "retrieved_at_unix_seconds": RETRIEVED_AT
                },
                "note": MAPPING_NOTE
            })
        );
    }

    #[test]
    fn missing_metadata_is_omitted_rather_than_zero_or_false() {
        let request = ItemSearchInput {
            item_id: Some(3),
            name: None,
            limit: None,
        }
        .into_request()
        .unwrap();

        let output = run_item_search(&sample_mapping(), &request);
        let json = serde_json::to_value(&output).unwrap();

        assert_eq!(
            json["matches"][0],
            json!({ "item_id": 3, "name": "Granite cannonball", "match_kind": "id" })
        );
    }

    #[tokio::test]
    async fn tool_is_discoverable_read_only_and_describes_its_input_and_output() {
        let market = FakeMarket::new(Ok(sample_mapping()));
        let client = connect(&market).await;

        let tools = client.list_all_tools().await.expect("tools/list");

        let tool = tools
            .iter()
            .find(|tool| tool.name == "item_search")
            .expect("item_search is registered");
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        let input = Value::Object((*tool.input_schema).clone());
        let properties = &input["properties"];
        for field in ["item_id", "name", "limit"] {
            assert!(
                properties[field].is_object(),
                "input field {field}: {input}"
            );
        }
        assert!(tool.output_schema.is_some(), "typed output schema");
        assert_eq!(market.requests(), 0, "discovery must not fetch data");
    }

    #[tokio::test]
    async fn name_search_returns_typed_structured_content_after_one_request() {
        let market = FakeMarket::new(Ok(sample_mapping()));
        let client = connect(&market).await;

        let result = call_item_search(&client, json!({ "name": "CANNONBALL", "limit": 1 })).await;

        assert_ne!(result.is_error, Some(true), "result: {result:?}");
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["matches"][0]["item_id"], 2);
        assert_eq!(structured["total_matches"], 2);
        assert_eq!(structured["truncated"], true);
        assert_eq!(structured["source"]["world_type"], "hardcore");
        assert_eq!(market.requests(), 1);
    }

    #[tokio::test]
    async fn invalid_input_is_a_tool_error_and_makes_no_request() {
        let market = FakeMarket::new(Ok(sample_mapping()));
        let client = connect(&market).await;

        for arguments in [
            json!({}),
            json!({ "item_id": 1, "name": "ab" }),
            json!({ "name": "x" }),
            json!({ "name": "ab", "limit": 26 }),
        ] {
            let result = call_item_search(&client, arguments).await;
            assert_eq!(result.is_error, Some(true), "result: {result:?}");
        }

        assert_eq!(market.requests(), 0);
    }

    #[tokio::test]
    async fn upstream_failure_is_a_safe_tool_error_distinct_from_no_matches() {
        let failure = MarketDataError::Http {
            endpoint: Endpoint::Mapping,
            status: 429,
        };
        let market = FakeMarket::new(Err(failure));
        let client = connect(&market).await;

        let result = call_item_search(&client, json!({ "name": "cannonball" })).await;

        assert_eq!(
            error_text(&result),
            "Hardcore mapping endpoint returned HTTP 429"
        );
        assert_eq!(market.requests(), 1, "no retry");
    }

    #[tokio::test]
    async fn unmatched_search_succeeds_with_an_empty_list() {
        let market = FakeMarket::new(Ok(sample_mapping()));
        let client = connect(&market).await;

        let result = call_item_search(&client, json!({ "name": "zzzz" })).await;

        assert_ne!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["matches"], json!([]));
        assert_eq!(structured["total_matches"], 0);
    }
}
