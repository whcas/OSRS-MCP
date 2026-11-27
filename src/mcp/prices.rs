//! The `item_prices` tool: latest observed Hardcore Grand Exchange trades.
//!
//! The adapter validates input, makes at most one `latest` request through
//! [`MarketData`], and converts the pure assessment from [`crate::market`] into
//! tool output. Missing, stale, future-dated and crossed data are reported, not
//! hidden; only failures to obtain data are errors.

use std::collections::BTreeMap;

use rmcp::{Json, handler::server::wrapper::Parameters, schemars::JsonSchema, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::OsrsMcpServer;
use super::provenance::SourceOutput;
use crate::domain::ItemId;
use crate::market::{
    LatestAssessment, LatestPrices, MaxAge, MaxAgeError, ObservationAge, Observed, SideAssessment,
    assess_latest,
};

/// Shown with every result so observations are not read as executable offers.
const PRICES_NOTE: &str = "high is the last observed instant-buy trade and low is the last \
observed instant-sell trade. They are observations, not guaranteed executable offers or fill \
prices. A null side means no observation was returned for it, not proof that it never traded. \
Ages are measured at the retrieval time, not the current time.";

/// Arguments of the `item_prices` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct ItemPricesInput {
    /// The item id, for example from `item_search`.
    item_id: u32,
    /// How old an observation may be, in whole seconds, and still be reported as
    /// fresh. Inclusive. 0 to 2592000 (30 days); defaults to 3600. Older
    /// observations are still returned, marked stale.
    max_age_seconds: Option<u64>,
}

/// A validated `item_prices` request.
struct ItemPricesRequest {
    item: ItemId,
    max_age: MaxAge,
}

impl ItemPricesInput {
    /// Checks the arguments. Nothing is fetched for an invalid request.
    fn into_request(self) -> Result<ItemPricesRequest, MaxAgeError> {
        let max_age = self
            .max_age_seconds
            .map_or(Ok(MaxAge::DEFAULT), MaxAge::new)?;
        Ok(ItemPricesRequest {
            item: ItemId(self.item_id),
            max_age,
        })
    }
}

/// Whether an observation is within the age limit.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum FreshnessOutput {
    /// Age is within the limit.
    Fresh,
    /// Older than the limit.
    Stale,
    /// Dated after the retrieval time: clock skew or bad data, never fresh.
    Future,
}

/// One side's last observed trade.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SideOutput {
    /// Price in whole GP. Zero is a real observed price.
    price_gp: u64,
    /// When the trade was observed, in whole seconds since the Unix epoch (UTC).
    observed_at_unix_seconds: u64,
    /// Whether this side is within the age limit. Judged for each side alone.
    freshness: FreshnessOutput,
    /// Age in whole seconds at the retrieval time. Present for `fresh` and `stale`.
    #[serde(skip_serializing_if = "Option::is_none")]
    age_seconds: Option<u64>,
    /// How far after the retrieval time the observation is, in whole seconds.
    /// Present only for `future`.
    #[serde(skip_serializing_if = "Option::is_none")]
    ahead_seconds: Option<u64>,
}

impl From<SideAssessment> for SideOutput {
    fn from(side: SideAssessment) -> Self {
        let (freshness, age_seconds, ahead_seconds) = match side.age {
            ObservationAge::Fresh { age_seconds } => {
                (FreshnessOutput::Fresh, Some(age_seconds), None)
            }
            ObservationAge::Stale { age_seconds } => {
                (FreshnessOutput::Stale, Some(age_seconds), None)
            }
            ObservationAge::Future { ahead_seconds } => {
                (FreshnessOutput::Future, None, Some(ahead_seconds))
            }
        };
        Self {
            price_gp: side.price.0,
            observed_at_unix_seconds: side.observed_at.0,
            freshness,
            age_seconds,
            ahead_seconds,
        }
    }
}

/// Result of the `item_prices` tool.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ItemPricesOutput {
    /// The requested item id.
    item_id: u32,
    /// Whether the item appeared in the latest-prices response. This does not
    /// establish that the item is listed or tradable on the Grand Exchange.
    present_in_latest: bool,
    /// Last observed instant-buy trade, or null if none was returned.
    high: Option<SideOutput>,
    /// Last observed instant-sell trade, or null if none was returned.
    low: Option<SideOutput>,
    /// Present only when both sides were returned: whether `low` is above
    /// `high`. Both prices are reported exactly as observed either way.
    #[serde(skip_serializing_if = "Option::is_none")]
    crossed: Option<bool>,
    /// The age limit applied, in whole seconds.
    max_age_seconds: u64,
    /// The reference time ages were measured at: the retrieval time, in whole
    /// seconds since the Unix epoch (UTC).
    evaluated_at_unix_seconds: u64,
    /// Where and when the prices were retrieved.
    source: SourceOutput,
    /// What the observations do and do not mean.
    note: &'static str,
}

fn item_prices_output(
    request: &ItemPricesRequest,
    latest: &Observed<BTreeMap<ItemId, LatestPrices>>,
) -> ItemPricesOutput {
    let provenance = &latest.provenance;
    let LatestAssessment {
        present_in_latest,
        high,
        low,
        crossed,
    } = assess_latest(
        latest.value.get(&request.item),
        provenance.retrieved_at,
        request.max_age,
    );
    ItemPricesOutput {
        item_id: request.item.0,
        present_in_latest,
        high: high.map(Into::into),
        low: low.map(Into::into),
        crossed,
        max_age_seconds: request.max_age.seconds(),
        evaluated_at_unix_seconds: provenance.retrieved_at.0,
        source: provenance.into(),
        note: PRICES_NOTE,
    }
}

#[tool_router(router = prices_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Reports the latest observed Hardcore trades for one item.
    #[tool(
        name = "item_prices",
        description = "Latest observed Hardcore Grand Exchange trades for one item id: the last \
                       instant-buy (high) and instant-sell (low) price, each with its trade \
                       time, age and freshness against a limit you can set, plus source and \
                       retrieval time. Missing, stale, future-dated and crossed data are \
                       reported, not hidden. Makes one request to the Hardcore prices API; \
                       never uses standard-world data. Observations are not guaranteed \
                       executable offers.",
        annotations(
            title = "Latest item prices",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn item_prices(
        &self,
        Parameters(input): Parameters<ItemPricesInput>,
    ) -> Result<Json<ItemPricesOutput>, String> {
        let request = input.into_request().map_err(|error| error.to_string())?;
        let latest = self
            .market
            .latest(request.item)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(item_prices_output(&request, &latest)))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::test_support::{FakeMarket, call_tool, connect, error_text, provenance};
    use super::*;
    use crate::domain::{Price, UnixSeconds};
    use crate::market::{Endpoint, MarketDataError, TimedPrice};

    const RETRIEVED_AT: u64 = 1_700_000_000;
    const ITEM: ItemId = ItemId(2);

    fn timed(price: u64, observed_at: u64) -> TimedPrice {
        TimedPrice {
            price: Price(price),
            observed_at: UnixSeconds(observed_at),
        }
    }

    fn latest_with(entry: Option<LatestPrices>) -> Observed<BTreeMap<ItemId, LatestPrices>> {
        Observed {
            value: entry.map(|prices| (ITEM, prices)).into_iter().collect(),
            provenance: provenance(Endpoint::Latest, RETRIEVED_AT),
        }
    }

    fn render(entry: Option<LatestPrices>, max_age_seconds: Option<u64>) -> Value {
        let request = ItemPricesInput {
            item_id: ITEM.0,
            max_age_seconds,
        }
        .into_request()
        .unwrap();
        serde_json::to_value(item_prices_output(&request, &latest_with(entry))).unwrap()
    }

    #[test]
    fn sides_report_price_time_age_and_freshness_with_provenance() {
        let json = render(
            Some(LatestPrices {
                high: Some(timed(150, RETRIEVED_AT - 60)),
                low: Some(timed(140, RETRIEVED_AT - 120)),
            }),
            None,
        );

        assert_eq!(
            json,
            json!({
                "item_id": 2,
                "present_in_latest": true,
                "high": {
                    "price_gp": 150,
                    "observed_at_unix_seconds": RETRIEVED_AT - 60,
                    "freshness": "fresh",
                    "age_seconds": 60
                },
                "low": {
                    "price_gp": 140,
                    "observed_at_unix_seconds": RETRIEVED_AT - 120,
                    "freshness": "fresh",
                    "age_seconds": 120
                },
                "crossed": false,
                "max_age_seconds": 3_600,
                "evaluated_at_unix_seconds": RETRIEVED_AT,
                "source": {
                    "world_type": "hardcore",
                    "endpoint": "latest",
                    "source_url": "https://example.invalid/latest",
                    "retrieved_at_unix_seconds": RETRIEVED_AT
                },
                "note": PRICES_NOTE
            })
        );
    }

    #[test]
    fn stale_and_future_sides_are_reported_per_side_not_discarded() {
        let json = render(
            Some(LatestPrices {
                high: Some(timed(150, RETRIEVED_AT - 7_200)),
                low: Some(timed(140, RETRIEVED_AT + 90)),
            }),
            None,
        );

        assert_eq!(json["high"]["freshness"], "stale");
        assert_eq!(json["high"]["age_seconds"], 7_200);
        assert_eq!(json["high"]["price_gp"], 150);
        assert_eq!(json["low"]["freshness"], "future");
        assert_eq!(json["low"]["ahead_seconds"], 90);
        assert!(json["low"].get("age_seconds").is_none());
    }

    #[test]
    fn a_per_request_age_limit_changes_the_verdict_and_is_echoed() {
        let entry = LatestPrices {
            high: Some(timed(150, RETRIEVED_AT - 7_200)),
            low: None,
        };

        let json = render(Some(entry), Some(86_400));

        assert_eq!(json["high"]["freshness"], "fresh");
        assert_eq!(json["max_age_seconds"], 86_400);
    }

    #[test]
    fn a_missing_side_is_null_and_a_zero_price_stays_zero() {
        let json = render(
            Some(LatestPrices {
                high: None,
                low: Some(timed(0, RETRIEVED_AT)),
            }),
            None,
        );

        assert_eq!(json["present_in_latest"], true);
        assert_eq!(json["high"], Value::Null);
        assert_eq!(json["low"]["price_gp"], 0);
        assert!(json.get("crossed").is_none(), "crossing needs both sides");
    }

    #[test]
    fn crossed_prices_are_flagged_and_both_kept_unchanged() {
        let json = render(
            Some(LatestPrices {
                high: Some(timed(100, RETRIEVED_AT)),
                low: Some(timed(120, RETRIEVED_AT)),
            }),
            None,
        );

        assert_eq!(json["crossed"], true);
        assert_eq!(json["high"]["price_gp"], 100);
        assert_eq!(json["low"]["price_gp"], 120);
    }

    #[test]
    fn an_omitted_item_is_distinct_from_one_with_no_observations() {
        let omitted = render(None, None);
        let empty = render(
            Some(LatestPrices {
                high: None,
                low: None,
            }),
            None,
        );

        assert_eq!(omitted["present_in_latest"], false);
        assert_eq!(empty["present_in_latest"], true);
        for json in [&omitted, &empty] {
            assert_eq!(json["high"], Value::Null);
            assert_eq!(json["low"], Value::Null);
        }
    }

    #[test]
    fn the_age_limit_is_bounded_to_thirty_days() {
        let reject = |seconds| {
            ItemPricesInput {
                item_id: 1,
                max_age_seconds: Some(seconds),
            }
            .into_request()
            .err()
        };

        assert_eq!(reject(2_592_001), Some(MaxAgeError));
        assert_eq!(reject(u64::MAX), Some(MaxAgeError));
        assert_eq!(reject(2_592_000), None);
        assert_eq!(reject(0), None);
    }

    #[tokio::test]
    async fn tool_is_discoverable_read_only_and_describes_its_input_and_output() {
        let market = FakeMarket::with_latest(Ok(latest_with(None)));
        let client = connect(&market).await;

        let tools = client.list_all_tools().await.expect("tools/list");

        let tool = tools
            .iter()
            .find(|tool| tool.name == "item_prices")
            .expect("item_prices is registered");
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        let input = Value::Object((*tool.input_schema).clone());
        assert_eq!(input["required"], json!(["item_id"]));
        assert!(input["properties"]["max_age_seconds"].is_object());
        assert!(tool.output_schema.is_some(), "typed output schema");
        assert_eq!(market.requests(), 0, "discovery must not fetch data");
    }

    #[tokio::test]
    async fn a_valid_call_returns_structured_content_after_exactly_one_request() {
        let entry = LatestPrices {
            high: Some(timed(150, RETRIEVED_AT - 10)),
            low: Some(timed(140, RETRIEVED_AT - 20)),
        };
        let market = FakeMarket::with_latest(Ok(latest_with(Some(entry))));
        let client = connect(&market).await;

        let result = call_tool(&client, "item_prices", json!({ "item_id": 2 })).await;

        assert_ne!(result.is_error, Some(true), "result: {result:?}");
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["high"]["price_gp"], 150);
        assert_eq!(structured["source"]["world_type"], "hardcore");
        assert_eq!(market.requests(), 1);
    }

    #[tokio::test]
    async fn invalid_input_is_a_tool_error_and_makes_no_request() {
        let market = FakeMarket::with_latest(Ok(latest_with(None)));
        let client = connect(&market).await;

        for arguments in [
            json!({}),
            json!({ "item_id": 2, "max_age_seconds": 2_592_001 }),
            json!({ "item_id": -1 }),
            json!({ "item_id": 2, "extra": true }),
        ] {
            let result = call_tool(&client, "item_prices", arguments).await;
            assert_eq!(result.is_error, Some(true), "result: {result:?}");
        }

        assert_eq!(market.requests(), 0);
    }

    #[tokio::test]
    async fn upstream_failure_is_a_safe_tool_error_distinct_from_missing_data() {
        let failure = MarketDataError::ResponseTooLarge {
            endpoint: Endpoint::Latest,
            limit_bytes: 1_024,
        };
        let market = FakeMarket::with_latest(Err(failure));
        let client = connect(&market).await;

        let result = call_tool(&client, "item_prices", json!({ "item_id": 2 })).await;

        assert_eq!(
            error_text(&result),
            "Hardcore latest response exceeded 1024 bytes"
        );
        assert_eq!(market.requests(), 1, "no retry");
    }
}
