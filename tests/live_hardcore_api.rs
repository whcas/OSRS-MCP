//! Opt-in contract check of `HardcoreClient` against the real Hardcore API.
//!
//! These tests make real network requests, so they never run by default or in
//! CI. Both gates must be open:
//!
//! 1. the test is `#[ignore]`d, so `cargo test` skips it;
//! 2. it returns immediately unless `OSRS_MCP_LIVE_API=1` is set.
//!
//! Run it deliberately and by hand:
//!
//! ```sh
//! OSRS_MCP_LIVE_API=1 cargo test --locked --test live_hardcore_api -- --ignored --nocapture
//! ```
//!
//! It uses the client's default configuration (the project `User-Agent`, the
//! real `osrs-hc` endpoint), one request per endpoint, sequentially, with a
//! pause between requests and no retries. It asserts only structural facts
//! (never specific prices), and prints counts rather than response data. No
//! response body is written to disk.

use std::time::Duration;

use osrs_mcp::market::{
    ClientConfig, Endpoint, HARDCORE_BASE_URL, HardcoreClient, ItemId, Lookback, Observed,
    UnixSeconds, WindowLength, WorldType,
};

const ENABLE_VAR: &str = "OSRS_MCP_LIVE_API";
/// Deliberately slow: one request every two seconds, to stay well inside the
/// API operator's usage expectations.
const PAUSE_BETWEEN_REQUESTS: Duration = Duration::from_secs(2);
const SAMPLE_ITEM: ItemId = ItemId(4151);
/// Any plausible current Unix time is after this (2023-11-14); it catches a
/// zero or millisecond-scaled `retrieved_at`.
const EARLIEST_PLAUSIBLE_SECONDS: u64 = 1_700_000_000;

fn live_tests_enabled() -> bool {
    std::env::var(ENABLE_VAR).is_ok_and(|value| value == "1")
}

fn assert_hardcore<T>(observed: &Observed<T>, endpoint: Endpoint) {
    let provenance = &observed.provenance;
    assert_eq!(provenance.world_type, WorldType::Hardcore);
    assert_eq!(provenance.endpoint, endpoint);
    assert!(
        provenance.source_url.starts_with(HARDCORE_BASE_URL),
        "source URL was not the Hardcore endpoint: {}",
        provenance.source_url
    );
    assert!(provenance.retrieved_at.0 > EARLIEST_PLAUSIBLE_SECONDS);
}

async fn pause() {
    tokio::time::sleep(PAUSE_BETWEEN_REQUESTS).await;
}

#[tokio::test]
#[ignore = "makes real network requests; run with OSRS_MCP_LIVE_API=1 and --ignored"]
async fn the_real_hardcore_api_matches_the_typed_client() {
    if !live_tests_enabled() {
        println!("skipped: set {ENABLE_VAR}=1 to run the live Hardcore API check");
        return;
    }
    let client = HardcoreClient::new(ClientConfig::default()).expect("default config is valid");

    let mapping = client.mapping().await.expect("mapping");
    assert_hardcore(&mapping, Endpoint::Mapping);
    assert!(!mapping.value.is_empty(), "mapping returned no items");
    assert!(
        mapping.value.iter().any(|item| item.id == SAMPLE_ITEM),
        "sample item missing from mapping"
    );
    println!("mapping: {} items", mapping.value.len());
    pause().await;

    let latest = client.latest(Some(SAMPLE_ITEM)).await.expect("latest");
    assert_hardcore(&latest, Endpoint::Latest);
    assert!(
        latest.value.len() <= 1,
        "scoped latest returned other items"
    );
    println!("latest: {} item(s) for the sample", latest.value.len());
    pause().await;

    let five_minute = client
        .window(WindowLength::FiveMinutes, None)
        .await
        .expect("5m window");
    assert_hardcore(&five_minute, Endpoint::FiveMinute);
    let start = five_minute.value.starts_at.expect("5m window has a start");
    assert_eq!(start.0 % 300, 0, "5m start is not on a 300 s boundary");
    println!("5m: {} items", five_minute.value.items.len());
    pause().await;

    let one_hour = client
        .window(WindowLength::OneHour, None)
        .await
        .expect("1h window");
    assert_hardcore(&one_hour, Endpoint::OneHour);
    let start: UnixSeconds = one_hour.value.starts_at.expect("1h window has a start");
    assert_eq!(start.0 % 3600, 0, "1h start is not on a 3600 s boundary");
    println!("1h: {} items", one_hour.value.items.len());
    pause().await;

    let series = client
        .timeseries(SAMPLE_ITEM, Lookback::TwentyFourHours)
        .await
        .expect("timeseries");
    assert_hardcore(&series, Endpoint::Timeseries);
    assert_eq!(series.value.item, SAMPLE_ITEM);
    assert!(series.value.timestep_seconds > 0);
    assert!(series.value.starts_at <= series.value.ends_at);
    println!(
        "timeseries: {} points, timestep {} s",
        series.value.points.len(),
        series.value.timestep_seconds
    );
}
