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

/// Converts a JSON number to hundredths of a GP without floating-point
/// rounding. More than two decimals, negatives and exponents are rejected
/// rather than rounded.
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
pub(super) fn latest(body: &[u8]) -> Result<BTreeMap<ItemId, LatestPrices>, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Latest;
    let envelope: Envelope<BTreeMap<String, LatestEntry>> = from_json(ENDPOINT, body)?;
    envelope
        .data
        .into_iter()
        .map(|(key, entry)| {
            let prices = LatestPrices {
                high: timed_price(ENDPOINT, "high", entry.high, entry.high_time)?,
                low: timed_price(ENDPOINT, "low", entry.low, entry.low_time)?,
            };
            Ok((item_id(ENDPOINT, &key)?, prices))
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

/// Parses a `/timeseries` body.
pub(super) fn timeseries(body: &[u8]) -> Result<Series, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Timeseries;
    let parsed: SeriesBody = from_json(ENDPOINT, body)?;
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
        let items = latest(LATEST.as_bytes()).unwrap();
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
        let items = latest(br#"{"data":{}}"#).unwrap();
        assert!(items.is_empty());
        assert!(!latest(LATEST.as_bytes()).unwrap().contains_key(&ItemId(1)));
    }

    #[test]
    fn latest_rejects_a_price_without_its_time_and_vice_versa() {
        assert!(is_malformed(&latest(
            br#"{"data":{"1":{"high":5,"highTime":null,"low":null,"lowTime":null}}}"#
        )));
        assert!(is_malformed(&latest(
            br#"{"data":{"1":{"high":null,"highTime":5,"low":null,"lowTime":null}}}"#
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
        assert_eq!(parse("5795000").unwrap().hundredths, 579_500_000);
        assert!(is_malformed(&parse("1.234")));
        assert!(is_malformed(&parse("-1")));
        // serde_json reads exponent forms as f64 and prints them back; exact ones
        // are accepted, ones that print with an exponent are rejected.
        assert_eq!(parse("1e3").unwrap().hundredths, 100_000);
        assert!(is_malformed(&parse("1e21")));
        assert!(is_malformed(&parse("184467440737095517")));
    }

    #[test]
    fn timeseries_parses_header_and_points() {
        let series = timeseries(TIMESERIES.as_bytes()).unwrap();
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
        assert!(is_malformed(&latest(b"")));
        assert!(is_malformed(&latest(b"<html>blocked</html>")));
        assert!(is_malformed(&latest(br#"{"data":[]}"#)));
        assert!(is_malformed(&latest(br#"{"nodata":{}}"#)));
        assert!(is_malformed(&latest(
            br#"{"data":{"not-an-id":{"high":null,"highTime":null,"low":null,"lowTime":null}}}"#
        )));
        assert!(is_malformed(&latest(
            br#"{"data":{"1":{"high":-5,"highTime":1,"low":null,"lowTime":null}}}"#
        )));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{"avgHighPrice":"12","highPriceVolume":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#
        )));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{"avgHighPrice":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#
        )));
        assert!(is_malformed(&timeseries(br#"{"data":[]}"#)));
        assert!(is_malformed(&mapping(br#"{"id":1}"#)));
        assert!(is_malformed(&mapping(br#"[{"id":1}]"#)));
    }
}
