//! Pure parsing of API response bodies into typed results. No I/O.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Number;

use super::error::MarketDataError;
use super::model::{
    AveragePrice, Endpoint, ItemId, ItemInfo, LatestPrices, Price, PriceWindow, Series,
    SeriesPoint, SideStats, TimedPrice, UnixSeconds, WindowStats,
};

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestEntry {
    high: Option<u64>,
    high_time: Option<u64>,
    low: Option<u64>,
    low_time: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowEntry {
    avg_high_price: Option<Number>,
    high_price_volume: u64,
    avg_low_price: Option<Number>,
    low_price_volume: u64,
}

#[derive(Deserialize)]
struct WindowBody {
    data: BTreeMap<String, WindowEntry>,
    timestamp: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesPointEntry {
    timestamp: u64,
    avg_high_price: Option<Number>,
    high_price_volume: u64,
    avg_low_price: Option<Number>,
    low_price_volume: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesBody {
    data: Vec<SeriesPointEntry>,
    item_id: u32,
    start_timestamp: u64,
    end_timestamp: u64,
    timestep: u64,
}

#[derive(Deserialize)]
struct MappingEntry {
    id: u32,
    name: String,
    members: Option<bool>,
    limit: Option<u32>,
    value: Option<u64>,
    lowalch: Option<u64>,
    highalch: Option<u64>,
}

fn malformed(endpoint: Endpoint, reason: impl Into<String>) -> MarketDataError {
    MarketDataError::Malformed {
        endpoint,
        reason: reason.into(),
    }
}

fn from_json<'a, T: Deserialize<'a>>(
    endpoint: Endpoint,
    body: &'a [u8],
) -> Result<T, MarketDataError> {
    serde_json::from_slice(body).map_err(|error| malformed(endpoint, error.to_string()))
}

fn item_id(endpoint: Endpoint, key: &str) -> Result<ItemId, MarketDataError> {
    key.parse()
        .map(ItemId)
        .map_err(|_| malformed(endpoint, format!("item key {key:?} is not an item id")))
}

/// Converts a JSON number to hundredths of a GP from its exact text.
///
/// `serde_json` is built with `arbitrary_precision`, so a `Number` keeps the
/// literal as written and no floating-point rounding happens. More than two
/// decimals, negatives and exponent notation (`1e3`) are rejected rather than
/// rounded or interpreted.
fn average_price(endpoint: Endpoint, number: &Number) -> Result<AveragePrice, MarketDataError> {
    let text = number.to_string();
    let bad = || {
        malformed(
            endpoint,
            format!("average price {text} is not a non-negative amount with at most two decimals"),
        )
    };
    if text.contains(['-', 'e', 'E']) {
        return Err(bad());
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    if fraction.len() > 2 {
        return Err(bad());
    }
    let whole: u64 = whole.parse().map_err(|_| bad())?;
    let fraction: u64 = format!("{fraction:0<2}").parse().map_err(|_| bad())?;
    whole
        .checked_mul(100)
        .and_then(|hundreds| hundreds.checked_add(fraction))
        .map(|hundredths| AveragePrice { hundredths })
        .ok_or_else(bad)
}

fn side_stats(
    endpoint: Endpoint,
    average: Option<&Number>,
    volume: u64,
) -> Result<SideStats, MarketDataError> {
    Ok(SideStats {
        average_price: average.map(|n| average_price(endpoint, n)).transpose()?,
        volume,
    })
}

fn window_stats(endpoint: Endpoint, entry: &WindowEntry) -> Result<WindowStats, MarketDataError> {
    Ok(WindowStats {
        high: side_stats(
            endpoint,
            entry.avg_high_price.as_ref(),
            entry.high_price_volume,
        )?,
        low: side_stats(
            endpoint,
            entry.avg_low_price.as_ref(),
            entry.low_price_volume,
        )?,
    })
}

fn timed_price(
    endpoint: Endpoint,
    side: &str,
    price: Option<u64>,
    time: Option<u64>,
) -> Result<Option<TimedPrice>, MarketDataError> {
    match (price, time) {
        (Some(price), Some(time)) => Ok(Some(TimedPrice {
            price: Price(price),
            observed_at: UnixSeconds(time),
        })),
        (None, None) => Ok(None),
        _ => Err(malformed(
            endpoint,
            format!("{side} price and {side} time must be both present or both null"),
        )),
    }
}

/// Parses a `/latest` body. Items the API omits are absent from the map;
/// items present with a never-seen side have `None` for that side.
///
/// When `scope` names the requested item, any other item in the response is
/// malformed: an absent requested item is valid, a different one is not.
pub(super) fn latest(
    body: &[u8],
    scope: Option<ItemId>,
) -> Result<BTreeMap<ItemId, LatestPrices>, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Latest;
    let envelope: Envelope<BTreeMap<String, LatestEntry>> = from_json(ENDPOINT, body)?;
    envelope
        .data
        .into_iter()
        .map(|(key, entry)| {
            let id = item_id(ENDPOINT, &key)?;
            if scope.is_some_and(|requested| requested != id) {
                return Err(malformed(
                    ENDPOINT,
                    format!("response contains item {key} but a different item was requested"),
                ));
            }
            let prices = LatestPrices {
                high: timed_price(ENDPOINT, "high", entry.high, entry.high_time)?,
                low: timed_price(ENDPOINT, "low", entry.low, entry.low_time)?,
            };
            Ok((id, prices))
        })
        .collect()
}

/// Parses a `/5m` or `/1h` body.
pub(super) fn window(endpoint: Endpoint, body: &[u8]) -> Result<PriceWindow, MarketDataError> {
    let parsed: WindowBody = from_json(endpoint, body)?;
    let items = parsed
        .data
        .iter()
        .map(|(key, entry)| Ok((item_id(endpoint, key)?, window_stats(endpoint, entry)?)))
        .collect::<Result<_, MarketDataError>>()?;
    Ok(PriceWindow {
        starts_at: parsed.timestamp.map(UnixSeconds),
        items,
    })
}

/// Parses a `/timeseries` body for the `requested` item. A response for a
/// different item, a zero timestep or a start after the end is malformed.
pub(super) fn timeseries(body: &[u8], requested: ItemId) -> Result<Series, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Timeseries;
    let parsed: SeriesBody = from_json(ENDPOINT, body)?;
    if ItemId(parsed.item_id) != requested {
        return Err(malformed(
            ENDPOINT,
            format!(
                "response is for item {} but item {} was requested",
                parsed.item_id, requested.0
            ),
        ));
    }
    if parsed.timestep == 0 {
        return Err(malformed(ENDPOINT, "timestep is zero"));
    }
    if parsed.start_timestamp > parsed.end_timestamp {
        return Err(malformed(
            ENDPOINT,
            "start timestamp is after end timestamp",
        ));
    }
    let points = parsed
        .data
        .iter()
        .map(|point| {
            Ok(SeriesPoint {
                timestamp: UnixSeconds(point.timestamp),
                stats: WindowStats {
                    high: side_stats(
                        ENDPOINT,
                        point.avg_high_price.as_ref(),
                        point.high_price_volume,
                    )?,
                    low: side_stats(
                        ENDPOINT,
                        point.avg_low_price.as_ref(),
                        point.low_price_volume,
                    )?,
                },
            })
        })
        .collect::<Result<_, MarketDataError>>()?;
    Ok(Series {
        item: ItemId(parsed.item_id),
        starts_at: UnixSeconds(parsed.start_timestamp),
        ends_at: UnixSeconds(parsed.end_timestamp),
        timestep_seconds: parsed.timestep,
        points,
    })
}

/// Parses a `/mapping` body.
pub(super) fn mapping(body: &[u8]) -> Result<Vec<ItemInfo>, MarketDataError> {
    let entries: Vec<MappingEntry> = from_json(Endpoint::Mapping, body)?;
    Ok(entries
        .into_iter()
        .map(|entry| ItemInfo {
            id: ItemId(entry.id),
            name: entry.name,
            members: entry.members,
            buy_limit: entry.limit,
            value: entry.value,
            low_alchemy: entry.lowalch,
            high_alchemy: entry.highalch,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST: &str = include_str!("../../tests/fixtures/hardcore_ge/latest.json");
    const FIVE_MINUTE: &str = include_str!("../../tests/fixtures/hardcore_ge/five_minute.json");
    const TIMESERIES: &str = include_str!("../../tests/fixtures/hardcore_ge/timeseries_24h.json");
    const MAPPING: &str = include_str!("../../tests/fixtures/hardcore_ge/mapping.json");

    fn is_malformed<T>(result: &Result<T, MarketDataError>) -> bool {
        matches!(result, Err(MarketDataError::Malformed { .. }))
    }

    #[test]
    fn latest_keeps_one_sided_and_never_seen_items_distinct() {
        let items = latest(LATEST.as_bytes(), None).unwrap();
        assert_eq!(items.len(), 3);

        let high_only = items[&ItemId(4151)];
        assert_eq!(
            high_only.high,
            Some(TimedPrice {
                price: Price(8_000_000),
                observed_at: UnixSeconds(1_794_623_592)
            })
        );
        assert_eq!(high_only.low, None);

        let low_only = items[&ItemId(2)];
        assert_eq!(low_only.high, None);
        assert_eq!(low_only.low.map(|t| t.price), Some(Price(150)));

        let never_seen = items[&ItemId(99)];
        assert_eq!((never_seen.high, never_seen.low), (None, None));
    }

    #[test]
    fn latest_omits_items_the_api_omits() {
        let items = latest(br#"{"data":{}}"#, None).unwrap();
        assert!(items.is_empty());
        assert!(
            !latest(LATEST.as_bytes(), None)
                .unwrap()
                .contains_key(&ItemId(1))
        );
    }

    #[test]
    fn latest_rejects_a_price_without_its_time_and_vice_versa() {
        assert!(is_malformed(&latest(
            br#"{"data":{"1":{"high":5,"highTime":null,"low":null,"lowTime":null}}}"#,
            None
        )));
        assert!(is_malformed(&latest(
            br#"{"data":{"1":{"high":null,"highTime":5,"low":null,"lowTime":null}}}"#,
            None
        )));
    }

    #[test]
    fn window_preserves_null_averages_zero_volumes_and_decimals() {
        let window = window(Endpoint::FiveMinute, FIVE_MINUTE.as_bytes()).unwrap();
        assert_eq!(window.starts_at, Some(UnixSeconds(1_795_504_200)));
        assert_eq!(window.items.len(), 4);

        let sell_side_only = window.items[&ItemId(2)];
        assert_eq!(sell_side_only.high.average_price, None);
        assert_eq!(sell_side_only.high.volume, 0);
        assert_eq!(
            sell_side_only.low.average_price,
            Some(AveragePrice { hundredths: 22_722 })
        );
        assert_eq!(sell_side_only.low.volume, 35_600);

        let whole = window.items[&ItemId(30)];
        assert_eq!(
            whole.high.average_price,
            Some(AveragePrice {
                hundredths: 100_000
            })
        );

        let fractional = window.items[&ItemId(41)];
        assert_eq!(
            fractional.high.average_price.map(|a| a.hundredths),
            Some(1_250)
        );
        assert_eq!(
            fractional.low.average_price.map(|a| a.hundredths),
            Some(1_005)
        );
    }

    #[test]
    fn window_without_a_timestamp_reports_none_rather_than_inventing_one() {
        let window = window(Endpoint::OneHour, br#"{"data":{}}"#).unwrap();
        assert_eq!(window.starts_at, None);
    }

    #[test]
    fn average_prices_are_exact_or_rejected() {
        let parse = |text: &str| {
            let number: Number = serde_json::from_str(text).unwrap();
            average_price(Endpoint::OneHour, &number)
        };
        assert_eq!(parse("0").unwrap().hundredths, 0);
        assert_eq!(parse("0.07").unwrap().hundredths, 7);
        assert_eq!(parse("0.10").unwrap().hundredths, 10);
        assert_eq!(parse("227.22").unwrap().hundredths, 22_722);
        assert_eq!(parse("1000.0").unwrap().hundredths, 100_000);
        assert_eq!(parse("5795000").unwrap().hundredths, 579_500_000);
        assert!(is_malformed(&parse("1.234")));
        assert!(is_malformed(&parse("-1")));
        assert!(is_malformed(&parse("184467440737095517")));
    }

    #[test]
    fn over_precise_averages_are_not_silently_rounded_to_a_valid_value() {
        // As an f64 this literal is exactly 227.22, so without exact lexemes it
        // would be accepted as 22,722 hundredths.
        let body = br#"{"data":{"1":{"avgHighPrice":227.2200000000000000001,"highPriceVolume":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#;
        assert!(is_malformed(&window(Endpoint::FiveMinute, body)));
    }

    #[test]
    fn exponent_notation_is_rejected_not_interpreted() {
        let number: Number = serde_json::from_str("1e3").unwrap();
        assert!(is_malformed(&average_price(Endpoint::OneHour, &number)));
        let number: Number = serde_json::from_str("1.5E2").unwrap();
        assert!(is_malformed(&average_price(Endpoint::OneHour, &number)));
    }

    #[test]
    fn scoped_latest_accepts_the_requested_item_or_none_but_not_another() {
        let requested = Some(ItemId(4151));
        let items = latest(LATEST.as_bytes(), None).unwrap();
        assert_eq!(items.len(), 3, "unscoped responses may hold many items");

        let only_requested =
            br#"{"data":{"4151":{"high":1,"highTime":2,"low":null,"lowTime":null}}}"#;
        assert_eq!(latest(only_requested, requested).unwrap().len(), 1);

        let absent = br#"{"data":{}}"#;
        assert!(latest(absent, requested).unwrap().is_empty());

        assert!(is_malformed(&latest(LATEST.as_bytes(), requested)));
        let other = br#"{"data":{"2":{"high":1,"highTime":2,"low":null,"lowTime":null}}}"#;
        assert!(is_malformed(&latest(other, requested)));
    }

    #[test]
    fn timeseries_for_another_item_is_rejected() {
        assert!(is_malformed(&timeseries(TIMESERIES.as_bytes(), ItemId(2))));
    }

    #[test]
    fn timeseries_with_zero_timestep_or_reversed_range_is_rejected() {
        let with = |start: u64, end: u64, step: u64| {
            format!(
                r#"{{"data":[],"itemId":4151,"startTimestamp":{start},"endTimestamp":{end},"timestep":{step}}}"#
            )
        };
        assert!(timeseries(with(10, 20, 300).as_bytes(), ItemId(4151)).is_ok());
        assert!(timeseries(with(10, 10, 300).as_bytes(), ItemId(4151)).is_ok());
        assert!(is_malformed(&timeseries(
            with(10, 20, 0).as_bytes(),
            ItemId(4151)
        )));
        assert!(is_malformed(&timeseries(
            with(21, 20, 300).as_bytes(),
            ItemId(4151)
        )));
    }

    #[test]
    fn timeseries_parses_header_and_points() {
        let series = timeseries(TIMESERIES.as_bytes(), ItemId(4151)).unwrap();
        assert_eq!(series.item, ItemId(4151));
        assert_eq!(series.starts_at, UnixSeconds(1_795_602_300));
        assert_eq!(series.ends_at, UnixSeconds(1_795_688_700));
        assert_eq!(series.timestep_seconds, 300);
        assert_eq!(series.points.len(), 4);
        assert_eq!(series.points[0].stats.low.average_price, None);
        assert_eq!(series.points[0].stats.high.volume, 1);
        assert_eq!(
            series.points[2].stats.low.average_price,
            Some(AveragePrice {
                hundredths: 640_000_000
            })
        );
    }

    #[test]
    fn mapping_keeps_absent_optional_fields_as_none() {
        let items = mapping(MAPPING.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Example ammo");
        assert_eq!(items[0].buy_limit, Some(11_000));
        assert_eq!(items[0].members, Some(false));
        assert_eq!(items[1].buy_limit, None);
        assert_eq!(items[1].high_alchemy, None);
        assert_eq!(items[1].members, Some(true));
    }

    #[test]
    fn invalid_bodies_are_malformed_not_empty() {
        assert!(is_malformed(&latest(b"", None)));
        assert!(is_malformed(&latest(b"<html>blocked</html>", None)));
        assert!(is_malformed(&latest(br#"{"data":[]}"#, None)));
        assert!(is_malformed(&latest(br#"{"nodata":{}}"#, None)));
        assert!(is_malformed(&latest(
            br#"{"data":{"not-an-id":{"high":null,"highTime":null,"low":null,"lowTime":null}}}"#,
            None
        )));
        assert!(is_malformed(&latest(
            br#"{"data":{"1":{"high":-5,"highTime":1,"low":null,"lowTime":null}}}"#,
            None
        )));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{"avgHighPrice":"12","highPriceVolume":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#
        )));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{"avgHighPrice":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#
        )));
        assert!(is_malformed(&timeseries(br#"{"data":[]}"#, ItemId(4151))));
        assert!(is_malformed(&mapping(br#"{"id":1}"#)));
        assert!(is_malformed(&mapping(br#"[{"id":1}]"#)));
    }
}
