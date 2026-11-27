# Investigation: Hardcore Grand Exchange data source

- Issue: #13
- Observation date: 2026-11-26
- Status: Recommendation, pending maintainer review. Live-verified from an unrestricted network on 2026-11-26 (see "Live verification result"); item-versus-transaction volume and the aggregate side semantics remain unverified.

## Summary

Use the OSRS Wiki real-time prices API, **v2**, with the **`osrs-hc`** base path (`https://prices.runescape.wiki/api/v2/osrs-hc/`). It is documented as the Hardcore Worlds endpoint, and the sample responses differ from the standard `osrs` endpoint for the same item, so it is not an alias. No alternative Hardcore source was found. The data is thin: many items have a price on one side only, so missing values and low volumes must be first-class in the client (#18).

## Method and limits (the original, restricted investigation)

Direct HTTP requests from the development container were blocked by its egress proxy (`prices.runescape.wiki`, `oldschool.runescape.wiki` and `secure.runescape.com` all refused). The documentation and sample responses below were instead read through a web-fetch tool that returns page text and may truncate or summarise large bodies. Consequences:

- **Verified (one request each):** the six endpoints below returned HTTP 200 `application/json`; field names and value types come from bodies that were returned verbatim (`latest`, `5m`, `timeseries`).
- **Not verified:** response headers (`Cache-Control`, `Date`, rate-limit headers were not visible); the `1h` top-level `timestamp` and the `mapping` entry count and optional fields (those bodies were truncated by the tool); behaviour under our own `User-Agent`; error responses (4xx/5xx, rate limiting); that the fetch tool's UA was permitted in the same way ours will be.
- **Update 2026-11-26:** the opt-in script has since been run from the maintainer's Mac on an unrestricted network; see "Live verification result". The bullets above describe the original, restricted investigation and are kept for the record.

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

## Local verification procedure

One procedure, run by a maintainer from a normal machine. Nothing here is run by CI or `scripts/check.sh`.

```sh
OSRS_MCP_LIVE_API=1 cargo test --locked --test live_hardcore_api -- --ignored --nocapture
```

- `tests/live_hardcore_api.rs` is `#[ignore]`d and also needs `OSRS_MCP_LIVE_API=1`, so it cannot run by accident.
- It uses `ClientConfig::default()` (the project User-Agent `osrs-mcp/<Cargo version> (+https://github.com/whcas/OSRS-MCP)`, a 30 s timeout and an 8 MiB body cap), makes six sequential requests two seconds apart (`mapping`, `latest?id=4151`, `5m`, `5m` scoped to the start it just returned, `1h`, `timeseries` 24h) and never retries.
- A second ignored test, `the_real_bulk_latest_response_satisfies_the_required_field_rule`, makes one bulk `latest` request (run it alone, once, by name). Both assert structural facts only (Hardcore provenance, source URL, boundaries of the `5m` and `1h` timestamps, the sample item present, a positive timestep) and prints counts, never bodies. It writes nothing to disk.
- Last run 2026-11-26, with the object-only parser: passed (mapping 4682 items, `5m` 450, scoped `5m` 450, `1h` 1182, `timeseries` 65 points).
- A failure is a result: it means the real API or the client's expectations drifted. Record it, compare with this document, and fix the client or the document; do not retry in a loop. A non-200 status (for example 403 for a blocked User-Agent, or 429) is likewise a result.

The first live check used a throwaway Python script (`scripts/verify_hardcore_api.py`, since retired, see the git history of PR #24) that also printed response headers and compared one standard-world request. Its findings are recorded below. If header-level checks are needed again, expose them from the client instead of keeping a parallel script. This procedure does not settle whether volume counts items or transactions, or the side semantics of the aggregates; those need a source statement or a controlled observation.

Refreshing fixtures in `tests/fixtures/hardcore_ge/` (PR #29) is manual and small, after step 3 shows the schema holds:

```sh
UA="osrs-mcp/0.1.0 (+https://github.com/whcas/OSRS-MCP)"   # match Cargo.toml's version
curl -sS -A "$UA" 'https://prices.runescape.wiki/api/v2/osrs-hc/latest?id=4151'
curl -sS -A "$UA" 'https://prices.runescape.wiki/api/v2/osrs-hc/timeseries?id=4151&lookback=24h'
```

Trim bulk endpoints (`5m`, `1h`, `mapping`) by hand to a few entries, keep one entry with a null side, update the provenance table in the fixtures' README to say which are real captures and which synthetic, and commit only the trimmed files. Never commit a full response.

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
| `1h` top-level `timestamp`, `mapping` entry count and optional fields, response headers, User-Agent acceptance | **Observed** from the maintainer's Mac on 2026-11-26 with the project User-Agent; see "Live verification result" below |
| Window timestamp handling | **Observed (2026-11-26, 4 requests)**: `5m?timestamp=` with a 300 s-aligned value is echoed back exactly in the response `timestamp`. An unaligned value, a value one day in the future, and `1` each returned **HTTP 400** with a 46-byte JSON body of the form `{"error": ...}`. The client therefore requires a scoped response's timestamp to equal the requested one |
| Bulk `latest` (no `id`) | **Observed (2026-11-26, one request, real Rust client)**: 2868 items; every entry carried all four keys (`high`, `highTime`, `low`, `lowTime`), so the client's required-field rule holds. 502 entries were one-sided and none had neither side, so a never-traded item is absent from the bulk response rather than present with four nulls |
| Rate-limit headers, 429/5xx bodies | **Not observed**: no `X-RateLimit-*` or `Retry-After` header was present, and no 429 or 5xx was provoked |
| Hardcore GE fee | **Maintainer-observed, not from the API:** 0% in the in-game Convenience Fees UI on 2026-11-26 (#14, #20). The API says nothing about fees, so this is a limit of what the API tells us, not an unknown fee. Not independently verified against live trades |

## Live verification result (2026-11-26, 11:34 UTC)

The since-retired `scripts/verify_hardcore_api.py --compare-standard` was run locally (macOS, unrestricted network) with the project User-Agent `osrs-mcp/0.1.0 (+https://github.com/whcas/OSRS-MCP)`. Result: `6/6 requests verified`, all HTTP 200, `Content-Type: application/json; charset=utf-8`, so the project User-Agent is accepted. The schema matches the documented one; nothing in the earlier fetch-tool reading was contradicted.

| Observation | Value |
| --- | --- |
| `mapping` | 4682 entries, about 868 KB. `id`, `name`, `examine`, `icon`, `members` present on every entry. **Optional:** `limit` (4165 of 4682), `value` (4659), `highalch` and `lowalch` (4578 each) |
| `latest?id=4151` | `high`, `highTime`, `low`, `lowTime` all integers this time (the earlier fetch showed `low: null`), so one-sidedness changes over time |
| `5m` | 362 items; top-level `timestamp` 1795692300 (multiple of 300) |
| `1h` | 1058 items; top-level `timestamp` 1795683600 (multiple of 3600); present, so the earlier truncation hid it, it did not mean it was absent |
| `timeseries` 24h | 68 points for a 288-step window; **gaps between points** from 300 s up to 5400 s, so empty steps are omitted. Header `timestep` 300 |
| Null averages with non-zero volume | 0 across `5m`, `1h` and `timeseries` |
| Averages with more than two decimals, non-integer volumes | 0 |
| Headers | `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=5` on `mapping`; `public, must-revalidate, max-age=60` on `latest`; `Age` and `ETag` on most; **no rate-limit headers, no `Retry-After`, no `Last-Modified`** |
| Hardcore versus standard, item 4151 | Hardcore `latest` high 6,500,000 / low 6,528,000; standard 636,211 / 631,149. The two worlds differ by a factor of about ten |

Newly observed and relevant to #18:

- In the Hardcore `latest` sample, the instant-sell `low` (6,528,000) was **higher** than the instant-buy `high` (6,500,000), and their timestamps were about 26 minutes apart. A crossed pair is therefore possible and must not be treated as an error or silently corrected. Margin calculations should surface it.
- `highTime` was 2318 s old and `lowTime` 787 s old when sampled; standard-world times were 91 s and 39 s. Staleness is normal for Hardcore.

Still **inferred or unknown**, not settled by this run: item versus transaction volume, whether the aggregate `High`/`Low` sides match `latest`'s sides, the Hardcore GE fee (maintainer-observed at 0% in the in-game Convenience Fees UI on 2026-11-26, #14 and #20; the API itself is silent), and non-200 behaviour.

## Semantics

- **Buy/sell:** `high` is the latest *instant-buy* price and `low` the latest *instant-sell* price. For `5m`, `1h` and `timeseries`, `avgHighPrice`/`highPriceVolume` and `avgLowPrice`/`lowPriceVolume` are the corresponding averages and reported volumes. The documentation states this mapping only for `latest`; applying it to the aggregates is an inference.
- **Timestamps:** `highTime`/`lowTime` are Unix seconds of the trade. In `5m`/`1h` the timestamp is the **start** of the averaging block (so the block is only complete 5 minutes or 1 hour later). `timeseries` `timestep` is not guaranteed and may change.
- **Volume windows:** 5 minutes, 1 hour, and per-step values in `timeseries`; no other window is documented. Each is a reported volume per side (unit unverified: items or transactions are not distinguished), not a GP value.
- **Numbers:** prices can exceed `i32`. Averages can have up to two decimal places (e.g. `227.22`).
- **Missing values:** items never traded are absent from `latest`. A side never observed is `null` (for `latest`, both price and time). Aggregates use `null` average with volume `0`. Absent and `null` are different from a price of zero and must not be coerced.
- **Freshness:** not conveyed by the API beyond the timestamps. Hardcore items can have a last trade days old (`highTime` for item 4151 was about 12 days earlier than the standard sample), so staleness must be computed by the client from timestamps against an explicit threshold.
- **Fees/GE tax:** not documented by this API. The maintainer observed a 0% Hardcore fee in the in-game Convenience Fees UI on 2026-11-26 (#14, #20); it is kept as an explicit configurable input rather than inferred from the API, and has not been verified against live trades.

## Usage policy

- A descriptive `User-Agent` is the only stated requirement, ideally with contact information. Generic clients (`python-requests`, `curl`, `Java/*`, `Apache-HttpClient`, `RestSharp`, `Python-urllib`) are blocked pre-emptively. Proposed UA: `osrs-mcp/<version> (+https://github.com/whcas/OSRS-MCP)`. The project URL is the contact; no personal email or handle is used.
- No numeric rate limit. Access may be restricted if usage "threatens the stability" of the API, described as multiple large queries per second sustained. Fetch `/latest` without `id` rather than looping, and avoid polling; the bulk endpoints are cached.
- Content is CC BY-NC-SA 3.0 with additional terms; RuneScape marks belong to Jagex. No separate API attribution is stated. We should credit the OSRS Wiki as the source in tool output and docs, and not redistribute dumps of the data (also required by `AGENTS.md`).

## Client design (implemented in #18)

The proposal made during the investigation is implemented in `src/market`, whose module and type documentation is now the reference for the types, the error model and the dependencies; the earlier sketch is not repeated here because it no longer matches the code. The constraints it set still hold: independent of MCP and persistence, one GET per call with bounded size and explicit timeouts, results always labelled Hardcore with provenance, no variant or code path for falling back to standard-world data, and missing or stale data reported as such rather than as an error.

## Fixture strategy

- Small hand-trimmed JSON bodies under `tests/fixtures/hardcore_ge/`, derived from the samples above and then replaced with `curl`-captured bodies: a `latest` with a one-sided price, a `latest` for a never-traded item (absent key), a `5m` with `null` averages and zero volume, a `timeseries` with a null side, and a malformed body. No full dumps.
- Parsing tested directly against fixtures; client behaviour tested against a local in-process HTTP server (status errors, timeouts, oversized body, wrong world path). CI makes no live requests.
- An ignored, manually run test may hit the live API for contract drift checks; it is never part of `scripts/check.sh`.

## Implementation notes (#18)

The client in `src/market/` follows the sketch above with these differences:

- Item ids, prices, average prices and timestamps use the shared `domain` types (`ItemId`, `Price`, `AveragePrice`, `UnixSeconds`, from #39), not definitions of this module. The client does not convert prices to the signed `Gp` used by the profit primitives; `AveragePrice` has no conversion to a whole-GP price because rounding an average is an analysis decision.
- `reqwest` 0.13 puts query-string support behind a feature, so query parameters are built with `Url::query_pairs_mut`.
- The mapping response is parsed with only `id` and `name` required; the live check showed `limit`, `value`, `highalch` and `lowalch` can be absent, so the parser's choice is confirmed.
- Fixtures in `tests/fixtures/hardcore_ge/` are partly synthetic; see the README there.
- The client's endpoint is fixed to the real `osrs-hc` base URL, with no public way to change it, and it records the exact source URL in provenance, validates scoped responses against the request, and reports observations dated in the future as such. These follow review feedback and are covered by regression tests.
- The client was checked against the live API on 2026-11-26 with the opt-in test in "Local verification procedure". Only HTTP 400 for bad window timestamps has been observed among error responses; 403, 429 and 5xx remain unobserved.

## Trade-offs and recommendation

- **For `osrs-hc`:** the only documented Hardcore source, same schema as standard, free and unauthenticated, cached, and explicitly supported by the wiki.
- **Against:** sparse and stale items; one-sided prices; averages with decimals; the timestep and v1 may change; no fee information in the API (the maintainer observed 0% in game); coverage depends on Jagex-provided data since 2026-09-30.
- **Recommendation:** adopt `api/v2/osrs-hc`, build #18 against the schema above after a maintainer re-verifies the samples with the real User-Agent, treat staleness and one-sided prices as normal conditions, and keep tax/fee policy as a separate explicit input.

## Open questions

1. ~~Hardcore GE tax~~: maintainer-observed 0% in the in-game Convenience Fees UI on 2026-11-26 (#14, #20); not derivable from the API.
2. ~~Exact `1h` top-level `timestamp` and optional `mapping` fields~~: answered by the live verification result.
3. ~~Real response headers~~: observed. Rate-limit behaviour under load, and 403/429/5xx responses, remain unobserved; 400 for bad window timestamps is observed.
4. Whether `highPriceVolume` and `lowPriceVolume` count items or transactions.
5. Whether the aggregate endpoints' "high" and "low" sides match `latest`'s instant-buy and instant-sell sides.
