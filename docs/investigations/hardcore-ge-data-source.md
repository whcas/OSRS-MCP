# Investigation: Hardcore Grand Exchange data source

- Issue: #13
- Observation date: 2026-11-26
- Status: Recommendation, pending maintainer review

## Summary

Use the OSRS Wiki real-time prices API, **v2**, with the **`osrs-hc`** base path (`https://prices.runescape.wiki/api/v2/osrs-hc/`). It is documented as the Hardcore Worlds endpoint, and the sample responses differ from the standard `osrs` endpoint for the same item, so it is not an alias. No alternative Hardcore source was found. The data is thin: many items have a price on one side only, so missing values and low volumes must be first-class in the client (#18).

## Method and limits of this verification

Direct HTTP requests from the development container were blocked by its egress proxy (`prices.runescape.wiki`, `oldschool.runescape.wiki` and `secure.runescape.com` all refused). The documentation and sample responses below were instead read through a web-fetch tool that returns page text and may truncate or summarise large bodies. Consequences:

- **Verified (one request each):** the six endpoints below returned HTTP 200 `application/json`; field names and value types come from bodies that were returned verbatim (`latest`, `5m`, `timeseries`).
- **Not verified:** response headers (`Cache-Control`, `Date`, rate-limit headers were not visible); the `1h` top-level `timestamp` and the `mapping` entry count and optional fields (those bodies were truncated by the tool); behaviour under our own `User-Agent`; error responses (4xx/5xx, rate limiting); that the fetch tool's UA was permitted in the same way ours will be.
- **Live verification has not been done by the maintainer, and not by this investigation from an unrestricted network.** `scripts/verify_hardcore_api.py` is an opt-in script for that: it makes five sequential Hardcore requests (plus one optional standard-world comparison) with the project User-Agent, explicit timeouts and a size cap, and prints status, selected headers and compact schema summaries without saving any bodies. Its output should be pasted into the review once run. Until then, treat everything under "Verified" below as read through the fetch tool only.

## Sources compared

| Source | Hardcore data? | Finding |
| --- | --- | --- |
| [OSRS Wiki real-time prices documentation](https://oldschool.runescape.wiki/w/RuneScape:Real-time_Prices) (page last modified 2026-11-09) | **Yes**: documents `api/v2/osrs-hc` for Hardcore Worlds alongside `api/v2/osrs` | Primary source. No Seasonal, Deadman or Leagues endpoints are documented. v1 "is still available but may be removed", so build on v2. |
| Live responses from `prices.runescape.wiki/api/v2/osrs-hc/*` | Yes | Sampled, see below. |
| Live response from `prices.runescape.wiki/api/v2/osrs/latest` | No (standard world) | Used only to confirm the two endpoints differ. Must never be a fallback. |
| Official Jagex item database (`secure.runescape.com/m=itemdb_oldschool`) | Not verified | Could not be reached. Not evidenced to have any Hardcore variant; out of consideration without evidence. |
| RuneLite GE price service | Not verified | Not investigated. The wiki page states data before 2026-09-30 came from RuneLite only and newer data comes from Jagex and covers all clients. |

## Endpoints (v2, base `https://prices.runescape.wiki/api/v2/osrs-hc`)

| Path | Query | Response |
| --- | --- | --- |
| `/latest` | `id` optional (omit for all items; do not loop per item) | `{"data": {"<itemId>": {"high", "highTime", "low", "lowTime"}}}` |
| `/mapping` | none | JSON array of `{examine, id, members, lowalch, limit, value, highalch, icon, name}` |
| `/5m` | `timestamp` optional (Unix seconds, start of block) | `{"data": {"<itemId>": {"avgHighPrice", "highPriceVolume", "avgLowPrice", "lowPriceVolume"}}, "timestamp"}` |
| `/1h` | `timestamp` optional | same shape as `/5m` |
| `/timeseries` | `id` and `lookback` required; lookback one of `6h`, `24h`, `7d`, `30d`, `6m`, `1y` | `{"data": [{"timestamp", "avgHighPrice", "avgLowPrice", "highPriceVolume", "lowPriceVolume"}], "itemId", "startTimestamp", "endTimestamp", "timestep"}` |

Observed samples (verbatim, 2026-11-26):

```json
/osrs-hc/latest?id=4151   {"data":{"4151":{"high":8000000,"highTime":1794623592,"low":null,"lowTime":null}}}
/osrs/latest?id=4151      {"data":{"4151":{"high":644695,"highTime":1795605699,"low":627004,"lowTime":1795605702}}}
/osrs-hc/5m entries       "2":{"avgHighPrice":null,"highPriceVolume":0,"avgLowPrice":227.22,"lowPriceVolume":35600}
                          "30":{"avgHighPrice":1000,"highPriceVolume":1,"avgLowPrice":null,"lowPriceVolume":0}
/osrs-hc/timeseries 24h   {"timestamp":1795602300,"avgHighPrice":5795000,"avgLowPrice":null,"highPriceVolume":1,"lowPriceVolume":0}
                          top level: itemId 4151, startTimestamp 1795602300, endTimestamp 1795688700, timestep 300
```

The Hardcore `latest` price for item 4151 (8,000,000, last high seen about 12 days before the standard sample) against the standard 644,695 shows why standard-world data must never stand in for it.

## Verified versus inferred

| Claim | Status |
| --- | --- |
| Endpoint paths, query parameters and field names for `latest`, `5m` and `timeseries` | Directly observed in verbatim bodies (via the fetch tool) |
| `osrs-hc` returns different data from `osrs` for the same item | Directly observed for one item (4151) |
| `high` is instant-buy and `low` is instant-sell in `latest` | Stated by the documentation; not independently verifiable from a response |
| `avgHighPrice`/`highPriceVolume` correspond to the same instant-buy side as `high` | **Inferred** from naming; the documentation does not state it for the aggregate endpoints |
| Volume counts **items** versus **transactions** | **Unknown.** The documentation says only "number traded". Observed volumes such as `35600` for item 2 and `1` for a single-item sale are consistent with either. The opt-in script cannot settle this; it needs a source statement or a controlled observation |
| Timestamp precision | Values are about 1.79e9, which is Unix **seconds**, consistent with the documentation (observed). Sub-second precision, if any, is not exposed |
| `5m`/`1h` `timestamp` is the block start | Stated by the documentation; the observed `5m` value 1795504200 is a multiple of 300, which is consistent |
| `timeseries` points are contiguous at `timestep` | **Not so, as observed.** The header said 300, but the first two sampled points are 600 s apart (1795602300 then 1795602900) while the last two are 300 s apart, so steps with no trades appear to be omitted (inferred). Clients must not assume a gap-free series. The documentation also says the timestep is not guaranteed |
| Averages have at most two decimals | Stated by the documentation; the observed values (e.g. `227.22`) are consistent. Over-precise values are rejected rather than rounded by the client |
| `1h` top-level `timestamp`, optional `mapping` fields, response headers, error bodies, rate-limit headers | **Not observed** (truncated or invisible via the fetch tool) |
| Whether Hardcore trades are taxed | **Unknown**; not in the API |

## Semantics

- **Buy/sell:** `high` is the latest *instant-buy* price and `low` the latest *instant-sell* price. For `5m`, `1h` and `timeseries`, `avgHighPrice`/`highPriceVolume` and `avgLowPrice`/`lowPriceVolume` are the corresponding averages and trade counts. The documentation states this mapping only for `latest`; applying it to the aggregates is an inference.
- **Timestamps:** `highTime`/`lowTime` are Unix seconds of the trade. In `5m`/`1h` the timestamp is the **start** of the averaging block (so the block is only complete 5 minutes or 1 hour later). `timeseries` `timestep` is not guaranteed and may change.
- **Volume windows:** 5 minutes, 1 hour, and per-step counts in `timeseries`. No other window is documented. Volumes are trade counts per side, not GP value.
- **Numbers:** prices can exceed `i32`. Averages can have up to two decimal places (e.g. `227.22`).
- **Missing values:** items never traded are absent from `latest`. A side never observed is `null` (for `latest`, both price and time). Aggregates use `null` average with volume `0`. Absent and `null` are different from a price of zero and must not be coerced.
- **Freshness:** not conveyed by the API beyond the timestamps. Hardcore items can have a last trade days old (`highTime` for item 4151 was about 12 days earlier than the standard sample), so staleness must be computed by the client from timestamps against an explicit threshold.
- **Fees/GE tax:** not documented by this API. Whether and how Hardcore trades are taxed is unresolved and remains an explicit input to #14.

## Usage policy

- A descriptive `User-Agent` is the only stated requirement, ideally with contact information. Generic clients (`python-requests`, `curl`, `Java/*`, `Apache-HttpClient`, `RestSharp`, `Python-urllib`) are blocked pre-emptively. Proposed UA: `osrs-mcp/<version> (+https://github.com/whcas/OSRS-MCP)`. The project URL is the contact; no personal email or handle is used.
- No numeric rate limit. Access may be restricted if usage "threatens the stability" of the API, described as multiple large queries per second sustained. Fetch `/latest` without `id` rather than looping, and avoid polling; the bulk endpoints are cached.
- Content is CC BY-NC-SA 3.0 with additional terms; RuneScape marks belong to Jagex. No separate API attribution is stated. We should credit the OSRS Wiki as the source in tool output and docs, and not redistribute dumps of the data (also required by `AGENTS.md`).

## Proposed minimal Rust client (for #18)

Keep it independent of MCP and persistence. Sketch only; names are open to review.

```rust
/// Chosen at construction; there is no default and no fallback.
pub enum WorldType { Hardcore }          // Standard added only if a feature needs it

pub struct Provenance { world_type: WorldType, endpoint: Endpoint, retrieved_at: SystemTime }

pub struct Observed<T> { value: T, provenance: Provenance }

pub struct ItemId(u32);
pub struct Gp(i64);                       // reuse crate::profit::Gp
pub struct AveragePrice { hundredths: i64 }      // two decimal places, exact

pub struct LatestPrice { high: Option<Timed<Gp>>, low: Option<Timed<Gp>> }
pub struct WindowStats { avg_high: Option<AveragePrice>, high_volume: u64,
                         avg_low: Option<AveragePrice>,  low_volume: u64 }
pub struct Window { start: UnixSeconds, length: WindowLength }   // 5m or 1h

impl HardcoreClient {
    pub async fn mapping(&self) -> Result<Observed<Vec<ItemInfo>>, MarketDataError>;
    pub async fn latest(&self, item: Option<ItemId>) -> Result<Observed<BTreeMap<ItemId, LatestPrice>>, _>;
    pub async fn window(&self, length: WindowLength, at: Option<UnixSeconds>)
        -> Result<Observed<BTreeMap<ItemId, WindowStats>>, _>;
    pub async fn timeseries(&self, item: ItemId, lookback: Lookback) -> Result<Observed<Series>, _>;
}

/// Staleness is a pure function so it is testable without a clock.
pub fn classify_age(observed: UnixSeconds, now: UnixSeconds, max_age: Duration) -> Freshness; // Fresh | Stale
```

Error model, one enum, no variant for "fell back to standard":

- `Transport` (connect error, timeout, TLS)
- `Http { status }` (non-success, including 429 and 403 for a blocked UA)
- `Malformed { endpoint, reason }` (not JSON, wrong shape, negative or non-integer price, unknown `lookback`)
- `ResponseTooLarge { limit }` (bounded body handling)

Missing data is not an error; it is `Option`/absent in the typed result. Stale data is not an error either; it is reported by `classify_age` so callers can show it.

Dependencies, to justify in #18: `reqwest` (rustls, JSON off; parse with `serde_json` ourselves to keep the body bound), `serde`, `serde_json`, `tokio` (for the async client). Timeouts and a response size cap are set explicitly on the client.

## Fixture strategy

- Small hand-trimmed JSON bodies under `tests/fixtures/hardcore_ge/`, derived from the samples above and then replaced with `curl`-captured bodies: a `latest` with a one-sided price, a `latest` for a never-traded item (absent key), a `5m` with `null` averages and zero volume, a `timeseries` with a null side, and a malformed body. No full dumps.
- Parsing tested directly against fixtures; client behaviour tested against a local in-process HTTP server (status errors, timeouts, oversized body, wrong world path). CI makes no live requests.
- An ignored, manually run test may hit the live API for contract drift checks; it is never part of `scripts/check.sh`.

## Trade-offs and recommendation

- **For `osrs-hc`:** the only documented Hardcore source, same schema as standard, free and unauthenticated, cached, and explicitly supported by the wiki.
- **Against:** sparse and stale items; one-sided prices; averages with decimals; the timestep and v1 may change; no Hardcore tax information; coverage depends on Jagex-provided data since 2026-09-30.
- **Recommendation:** adopt `api/v2/osrs-hc`, build #18 against the schema above after a maintainer re-verifies the samples with the real User-Agent, treat staleness and one-sided prices as normal conditions, and keep tax/fee policy as a separate explicit input.

## Open questions

1. Hardcore GE tax: does it apply, at what rate, with what rounding and cap?
2. Exact `1h` top-level `timestamp` and optional fields in `mapping` for this world type.
3. Real response headers and rate-limit behaviour for our User-Agent.
4. Whether `highPriceVolume` and `lowPriceVolume` count items or transactions.
5. Whether the aggregate endpoints' "high" and "low" sides match `latest`'s instant-buy and instant-sell sides.
