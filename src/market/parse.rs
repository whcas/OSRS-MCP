//! Pure parsing of API response bodies into typed results. No I/O.
//!
//! Every JSON object is read through [`Object`], which only accepts a JSON
//! object (never a positional array) and keeps each field as raw text. Fields
//! are then read by name, so a missing field, an explicit `null` and a wrong
//! type are all distinguishable, and error values never contain response text.
//!
//! Policies, all checked by tests:
//!
//! - A key that is not the canonical decimal spelling of an item id (for
//!   example `"01"` or `"+1"`) is rejected, so two spellings can never
//!   collapse into one item.
//! - If the same key appears twice in one JSON object, the last value wins;
//!   JSON leaves this undefined and the API does not produce it.
//! - Unknown fields are ignored.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use crate::domain::{AveragePrice, ItemId, Price, UnixSeconds};

use super::error::MarketDataError;
use super::model::{
    Endpoint, ItemInfo, LatestPrices, PriceWindow, Series, SeriesPoint, SideStats, TimedPrice,
    WindowStats,
};

// Bound each returned name separately from the HTTP body and search result count.
const MAX_MAPPING_NAME_BYTES: usize = 256;

/// A JSON object whose fields are still raw text.
#[derive(Deserialize)]
#[serde(transparent)]
struct Object(BTreeMap<String, Box<RawValue>>);

impl Object {
    /// The field `name`, which must be present and not `null`.
    fn required<T: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        name: &'static str,
    ) -> Result<T, MarketDataError> {
        self.nullable(endpoint, name)?
            .ok_or_else(|| invalid_field(endpoint, name))
    }

    /// The field `name`, which must be present. An explicit `null` is `None`.
    fn nullable<T: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        name: &'static str,
    ) -> Result<Option<T>, MarketDataError> {
        let raw = self
            .0
            .get(name)
            .ok_or_else(|| missing_field(endpoint, name))?;
        serde_json::from_str(raw.get()).map_err(|_| invalid_field(endpoint, name))
    }

    /// The field `name`, which may be absent or `null`.
    fn optional<T: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        name: &'static str,
    ) -> Result<Option<T>, MarketDataError> {
        if self.0.contains_key(name) {
            self.nullable(endpoint, name)
        } else {
            Ok(None)
        }
    }

    /// The raw text of the field `name`, which must be present; `None` if it
    /// is `null`. Used for numbers that must be read exactly.
    fn nullable_raw(
        &self,
        endpoint: Endpoint,
        name: &'static str,
    ) -> Result<Option<&RawValue>, MarketDataError> {
        let raw = self
            .0
            .get(name)
            .ok_or_else(|| missing_field(endpoint, name))?;
        Ok((raw.get() != "null").then_some(raw))
    }
}

const fn malformed(
    endpoint: Endpoint,
    field: Option<&'static str>,
    reason: &'static str,
) -> MarketDataError {
    MarketDataError::Malformed {
        endpoint,
        field,
        reason,
    }
}

const fn missing_field(endpoint: Endpoint, field: &'static str) -> MarketDataError {
    malformed(endpoint, Some(field), "required field is missing")
}

const fn invalid_field(endpoint: Endpoint, field: &'static str) -> MarketDataError {
    malformed(
        endpoint,
        Some(field),
        "field has an unexpected type or value",
    )
}

/// Parses a whole body. The error says only whether the JSON was invalid or
/// had the wrong shape, never what the body contained.
fn from_json<T: DeserializeOwned>(endpoint: Endpoint, body: &[u8]) -> Result<T, MarketDataError> {
    serde_json::from_slice(body).map_err(|error| match error.classify() {
        serde_json::error::Category::Data => {
            malformed(endpoint, None, "body does not have the documented shape")
        }
        _ => malformed(endpoint, None, "body is not valid JSON"),
    })
}

/// Parses one nested value, such as an entry of `data`, that is known to be
/// valid JSON because the whole body parsed.
fn from_raw<T: DeserializeOwned>(endpoint: Endpoint, raw: &RawValue) -> Result<T, MarketDataError> {
    serde_json::from_str(raw.get())
        .map_err(|_| malformed(endpoint, None, "entry does not have the documented shape"))
}

/// An item id from an object key. Only the canonical decimal spelling is
/// accepted, so distinct keys cannot resolve to the same id.
fn item_id(endpoint: Endpoint, key: &str) -> Result<ItemId, MarketDataError> {
    key.parse::<u32>()
        .ok()
        .filter(|id| id.to_string() == key)
        .map(ItemId)
        .ok_or_else(|| malformed(endpoint, None, "item key is not a canonical item id"))
}

/// Converts a JSON number literal to hundredths of a GP from its exact text.
///
/// The literal is captured as a `RawValue`, so no floating-point rounding
/// happens. More than two decimals, negatives and exponent notation (`1e3`)
/// are rejected rather than rounded or interpreted.
fn average_price(
    endpoint: Endpoint,
    field: &'static str,
    number: &RawValue,
) -> Result<AveragePrice, MarketDataError> {
    let text = number.get();
    let bad = || invalid_field(endpoint, field);
    if text.contains(['-', 'e', 'E']) {
        return Err(bad());
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
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

/// Averages and volumes of one window entry, which `/5m`, `/1h` and each
/// `/timeseries` point share. All four fields must be present; only the
/// averages may be `null`.
fn window_stats(endpoint: Endpoint, entry: &Object) -> Result<WindowStats, MarketDataError> {
    let side =
        |average: &'static str, volume: &'static str| -> Result<SideStats, MarketDataError> {
            Ok(SideStats {
                average_price: entry
                    .nullable_raw(endpoint, average)?
                    .map(|number| average_price(endpoint, average, number))
                    .transpose()?,
                volume: entry.required(endpoint, volume)?,
            })
        };
    Ok(WindowStats {
        high: side("avgHighPrice", "highPriceVolume")?,
        low: side("avgLowPrice", "lowPriceVolume")?,
    })
}

/// One side of a `/latest` entry. The key pair must be present; a price and
/// its time must be both set or both `null`.
fn timed_price(
    endpoint: Endpoint,
    entry: &Object,
    (price_key, time_key): (&'static str, &'static str),
) -> Result<Option<TimedPrice>, MarketDataError> {
    let price: Option<u64> = entry.nullable(endpoint, price_key)?;
    let time: Option<u64> = entry.nullable(endpoint, time_key)?;
    match (price, time) {
        (Some(price), Some(time)) => Ok(Some(TimedPrice {
            price: Price(price),
            observed_at: UnixSeconds(time),
        })),
        (None, None) => Ok(None),
        _ => Err(malformed(
            endpoint,
            Some(price_key),
            "price and time must be both present or both null",
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
    let envelope: Object = from_json(ENDPOINT, body)?;
    let data: Object = envelope.required(ENDPOINT, "data")?;
    data.0
        .iter()
        .map(|(key, raw)| read_latest_entry(key, raw, scope))
        .collect()
}

/// One `data` entry of `/latest`: its item id, which must be canonical and the
/// requested item if one was requested, and both sides' prices.
fn read_latest_entry(
    key: &str,
    raw: &RawValue,
    scope: Option<ItemId>,
) -> Result<(ItemId, LatestPrices), MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Latest;
    let id = item_id(ENDPOINT, key)?;
    if scope.is_some_and(|requested| requested != id) {
        return Err(malformed(
            ENDPOINT,
            None,
            "response contains an item other than the one requested",
        ));
    }
    let entry: Object = from_raw(ENDPOINT, raw)?;
    let prices = LatestPrices {
        high: timed_price(ENDPOINT, &entry, ("high", "highTime"))?,
        low: timed_price(ENDPOINT, &entry, ("low", "lowTime"))?,
    };
    Ok((id, prices))
}

/// Parses a `/5m` or `/1h` body.
///
/// When a window start was `requested`, the response must carry exactly that
/// timestamp: the API echoes an aligned request and answers anything else with
/// HTTP 400, so a different value means the data is for another period.
pub(super) fn window(
    endpoint: Endpoint,
    body: &[u8],
    requested: Option<UnixSeconds>,
) -> Result<PriceWindow, MarketDataError> {
    let envelope: Object = from_json(endpoint, body)?;
    let data: Object = envelope.required(endpoint, "data")?;
    let starts_at = envelope.optional(endpoint, "timestamp")?.map(UnixSeconds);
    check_window_start(endpoint, starts_at, requested)?;
    let items = data
        .0
        .iter()
        .map(|(key, raw)| {
            let entry: Object = from_raw(endpoint, raw)?;
            Ok((item_id(endpoint, key)?, window_stats(endpoint, &entry)?))
        })
        .collect::<Result<_, MarketDataError>>()?;
    Ok(PriceWindow { starts_at, items })
}

/// A window requested by its start must come back with that start.
fn check_window_start(
    endpoint: Endpoint,
    starts_at: Option<UnixSeconds>,
    requested: Option<UnixSeconds>,
) -> Result<(), MarketDataError> {
    if requested.is_some() && starts_at != requested {
        return Err(malformed(
            endpoint,
            Some("timestamp"),
            "window start differs from the one requested",
        ));
    }
    Ok(())
}

/// Parses a `/timeseries` body for the `requested` item.
pub(super) fn timeseries(body: &[u8], requested: ItemId) -> Result<Series, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Timeseries;
    let envelope: Object = from_json(ENDPOINT, body)?;
    let header = read_series_header(&envelope, requested)?;
    let points = read_series_points(&envelope)?;
    Ok(Series {
        item: header.item,
        starts_at: header.starts_at,
        ends_at: header.ends_at,
        timestep_seconds: header.timestep_seconds,
        points,
    })
}

/// The header fields of a `/timeseries` body, validated: the response must be
/// for the requested item, the timestep must be positive and the range must not
/// run backwards.
struct SeriesHeader {
    item: ItemId,
    starts_at: UnixSeconds,
    ends_at: UnixSeconds,
    timestep_seconds: u64,
}

fn read_series_header(
    envelope: &Object,
    requested: ItemId,
) -> Result<SeriesHeader, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Timeseries;
    let item = ItemId(envelope.required(ENDPOINT, "itemId")?);
    let starts_at: u64 = envelope.required(ENDPOINT, "startTimestamp")?;
    let ends_at: u64 = envelope.required(ENDPOINT, "endTimestamp")?;
    let timestep_seconds: u64 = envelope.required(ENDPOINT, "timestep")?;
    if item != requested {
        return Err(malformed(
            ENDPOINT,
            Some("itemId"),
            "response is for a different item than requested",
        ));
    }
    if timestep_seconds == 0 {
        return Err(malformed(ENDPOINT, Some("timestep"), "timestep is zero"));
    }
    if starts_at > ends_at {
        return Err(malformed(
            ENDPOINT,
            Some("startTimestamp"),
            "start timestamp is after end timestamp",
        ));
    }
    Ok(SeriesHeader {
        item,
        starts_at: UnixSeconds(starts_at),
        ends_at: UnixSeconds(ends_at),
        timestep_seconds,
    })
}

/// The points of a `/timeseries` body, each with its timestamp and both sides'
/// statistics.
fn read_series_points(envelope: &Object) -> Result<Vec<SeriesPoint>, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Timeseries;
    envelope
        .required::<Vec<Box<RawValue>>>(ENDPOINT, "data")?
        .iter()
        .map(|raw| {
            let point: Object = from_raw(ENDPOINT, raw)?;
            Ok(SeriesPoint {
                timestamp: UnixSeconds(point.required(ENDPOINT, "timestamp")?),
                stats: window_stats(ENDPOINT, &point)?,
            })
        })
        .collect()
}

/// Parses a `/mapping` body. Only `id` and `name` are required; the other
/// fields were observed to be absent for some items. IDs must be unique and
/// names must fit the byte limit.
pub(super) fn mapping(body: &[u8]) -> Result<Vec<ItemInfo>, MarketDataError> {
    const ENDPOINT: Endpoint = Endpoint::Mapping;
    let entries: Vec<Object> = from_json(ENDPOINT, body)?;
    let mut ids = BTreeSet::new();
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = ItemId(entry.required(ENDPOINT, "id")?);
        if !ids.insert(id) {
            return Err(malformed(
                ENDPOINT,
                Some("id"),
                "item id appears more than once",
            ));
        }
        let name: String = entry.required(ENDPOINT, "name")?;
        if name.len() > MAX_MAPPING_NAME_BYTES {
            return Err(malformed(
                ENDPOINT,
                Some("name"),
                "item name exceeds 256 UTF-8 bytes",
            ));
        }
        items.push(ItemInfo {
            id,
            name,
            members: entry.optional(ENDPOINT, "members")?,
            buy_limit: entry.optional(ENDPOINT, "limit")?,
            value: entry.optional(ENDPOINT, "value")?,
            low_alchemy: entry.optional(ENDPOINT, "lowalch")?,
            high_alchemy: entry.optional(ENDPOINT, "highalch")?,
        });
    }
    Ok(items)
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
        let window = window(Endpoint::FiveMinute, FIVE_MINUTE.as_bytes(), None).unwrap();
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
        let window = window(Endpoint::OneHour, br#"{"data":{}}"#, None).unwrap();
        assert_eq!(window.starts_at, None);
    }

    #[test]
    fn average_prices_are_exact_or_rejected() {
        let parse = |text: &str| {
            let number: Box<RawValue> = serde_json::from_str(text).unwrap();
            average_price(Endpoint::OneHour, "avgHighPrice", &number)
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
        assert!(is_malformed(&window(Endpoint::FiveMinute, body, None)));
    }

    #[test]
    fn exponent_notation_is_rejected_not_interpreted() {
        let number: Box<RawValue> = serde_json::from_str("1e3").unwrap();
        assert!(is_malformed(&average_price(
            Endpoint::OneHour,
            "avgHighPrice",
            &number
        )));
        let number: Box<RawValue> = serde_json::from_str("1.5E2").unwrap();
        assert!(is_malformed(&average_price(
            Endpoint::OneHour,
            "avgHighPrice",
            &number
        )));
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
    fn latest_rejects_non_json_wrong_shapes_and_bad_entries() {
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
    }

    #[test]
    fn window_rejects_a_mistyped_or_missing_field() {
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{"avgHighPrice":"12","highPriceVolume":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#,
            None
        )));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{"avgHighPrice":1,"avgLowPrice":null,"lowPriceVolume":0}}}"#,
            None
        )));
    }

    #[test]
    fn timeseries_without_its_header_is_malformed() {
        assert!(is_malformed(&timeseries(br#"{"data":[]}"#, ItemId(4151))));
    }

    #[test]
    fn mapping_rejects_a_non_array_and_an_entry_without_a_name() {
        assert!(is_malformed(&mapping(br#"{"id":1}"#)));
        assert!(is_malformed(&mapping(br#"[{"id":1}]"#)));
    }

    #[test]
    fn mapping_rejects_duplicate_ids_with_a_sanitized_error() {
        let body = br#"[{"id":2,"name":"Example ammo"},{"id":2,"name":"Other ammo"}]"#;
        let error = mapping(body).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Hardcore mapping response was malformed: item id appears more than once (field `id`)"
        );
    }

    #[test]
    fn mapping_keeps_distinct_ids_with_the_same_name() {
        let body = br#"[{"id":2,"name":"Example ammo"},{"id":3,"name":"Example ammo"}]"#;
        let items = mapping(body).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, ItemId(2));
        assert_eq!(items[1].id, ItemId(3));
        assert_eq!(items[0].name, items[1].name);
    }

    #[test]
    fn mapping_limits_names_by_decoded_utf8_bytes() {
        let name = "é".repeat(128);
        let at_limit = format!(r#"[{{"id":2,"name":"{name}"}}]"#);
        let over_limit = format!(r#"[{{"id":2,"name":"{name}x"}}]"#);

        assert_eq!(mapping(at_limit.as_bytes()).unwrap()[0].name, name);
        assert_eq!(
            mapping(over_limit.as_bytes()).unwrap_err().to_string(),
            "Hardcore mapping response was malformed: item name exceeds 256 UTF-8 bytes (field `name`)"
        );
    }

    #[test]
    fn json_arrays_are_never_read_as_objects_by_position() {
        // A derived `Deserialize` would accept each of these positionally.
        assert!(is_malformed(&latest(
            br#"{"data":{"1":[1,2,null,null]}}"#,
            None
        )));
        assert!(is_malformed(&latest(br#"[{"1":{}}]"#, None)));
        assert!(is_malformed(&latest(b"[{}]", None)));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":[1,1,null,0]}}"#,
            None
        )));
        assert!(is_malformed(&window(Endpoint::OneHour, b"[{}]", None)));
        assert!(is_malformed(&timeseries(
            b"[[],4151,0,10,300]",
            ItemId(4151)
        )));
        let point_as_array = br#"{"data":[[1,1,null,0,null]],"itemId":4151,"startTimestamp":1,"endTimestamp":2,"timestep":300}"#;
        assert!(is_malformed(&timeseries(point_as_array, ItemId(4151))));
        assert!(is_malformed(&mapping(br#"[[1,"name"]]"#)));
    }

    #[test]
    fn an_empty_latest_entry_is_not_a_never_observed_item() {
        // A never-observed item has all four keys with explicit nulls; an empty
        // object is a malformed response, not the same thing.
        assert!(is_malformed(&latest(br#"{"data":{"1":{}}}"#, None)));
        let missing_low_time = br#"{"data":{"1":{"high":null,"highTime":null,"low":null}}}"#;
        assert!(is_malformed(&latest(missing_low_time, None)));
        let explicit = br#"{"data":{"1":{"high":null,"highTime":null,"low":null,"lowTime":null}}}"#;
        assert_eq!(latest(explicit, None).unwrap()[&ItemId(1)].high, None);
    }

    #[test]
    fn window_entries_must_carry_all_four_fields_with_non_null_volumes() {
        let ok = br#"{"data":{"1":{"avgHighPrice":null,"highPriceVolume":0,"avgLowPrice":null,"lowPriceVolume":0}}}"#;
        assert!(window(Endpoint::FiveMinute, ok, None).is_ok());
        let no_average_key =
            br#"{"data":{"1":{"highPriceVolume":0,"avgLowPrice":null,"lowPriceVolume":0}}}"#;
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            no_average_key,
            None
        )));
        let null_volume = br#"{"data":{"1":{"avgHighPrice":null,"highPriceVolume":null,"avgLowPrice":null,"lowPriceVolume":0}}}"#;
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            null_volume,
            None
        )));
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{"1":{}}}"#,
            None
        )));
    }

    #[test]
    fn mapping_requires_id_and_name_and_tolerates_nulls_in_optional_fields() {
        let sparse = br#"[{"id":1,"name":"x","limit":null,"value":5}]"#;
        let items = mapping(sparse).unwrap();
        assert_eq!((items[0].buy_limit, items[0].value), (None, Some(5)));
        assert!(is_malformed(&mapping(br#"[{"name":"x"}]"#)));
        assert!(is_malformed(&mapping(br#"[{"id":1}]"#)));
        assert!(is_malformed(&mapping(br#"[{"id":1,"name":null}]"#)));
    }

    #[test]
    fn non_canonical_item_key_spellings_are_rejected() {
        let entry = r#"{"high":null,"highTime":null,"low":null,"lowTime":null}"#;
        for key in ["01", "+1", "1.0", " 1", "1 ", "0x1", "-1", "4294967296", ""] {
            let body = format!(r#"{{"data":{{"{key}":{entry}}}}}"#);
            assert!(is_malformed(&latest(body.as_bytes(), None)), "{key:?}");
        }
        let canonical = format!(r#"{{"data":{{"1":{entry},"10":{entry},"0":{entry}}}}}"#);
        assert_eq!(latest(canonical.as_bytes(), None).unwrap().len(), 3);
        // The same spelling twice cannot be told apart by id; the last value
        // wins, which is the documented policy.
        let repeated = br#"{"data":{"1":{"high":1,"highTime":1,"low":null,"lowTime":null},"1":{"high":2,"highTime":2,"low":null,"lowTime":null}}}"#;
        let items = latest(repeated, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[&ItemId(1)].high.map(|t| t.price), Some(Price(2)));
    }

    #[test]
    fn a_window_for_another_period_than_requested_is_rejected() {
        let body = br#"{"data":{},"timestamp":1795707600}"#;
        let same = Some(UnixSeconds(1_795_707_600));
        let other = Some(UnixSeconds(1_795_707_900));
        assert!(window(Endpoint::FiveMinute, body, same).is_ok());
        assert!(is_malformed(&window(Endpoint::FiveMinute, body, other)));
        // A requested start with no timestamp in the response is also unreliable.
        assert!(is_malformed(&window(
            Endpoint::FiveMinute,
            br#"{"data":{}}"#,
            same
        )));
        // Unscoped requests take whatever the API reports.
        assert!(window(Endpoint::FiveMinute, body, None).is_ok());
    }

    #[test]
    fn a_side_with_volume_but_no_average_is_flagged_not_hidden() {
        let body = br#"{"data":{"1":{"avgHighPrice":null,"highPriceVolume":3,"avgLowPrice":5,"lowPriceVolume":2}}}"#;
        let stats = window(Endpoint::FiveMinute, body, None).unwrap().items[&ItemId(1)];
        assert_eq!(stats.high.volume, 3, "the raw volume is preserved");
        assert!(stats.high.has_volume_without_average());
        assert!(!stats.low.has_volume_without_average());
    }

    #[test]
    fn error_text_is_bounded_whatever_the_response_contains() {
        let huge = "x".repeat(1_000_000);
        let cases = [
            format!(r#"{{"data":{{"{huge}":{{}}}}}}"#),
            format!(
                r#"{{"data":{{"1":{{"high":"{huge}","highTime":1,"low":null,"lowTime":null}}}}}}"#
            ),
            format!(
                r#"{{"data":{{"1":{{"avgHighPrice":1.{digits}"#,
                digits = "9".repeat(1_000_000)
            ),
            format!("{{\"data\":{huge}}}"),
            huge,
        ];
        for body in cases {
            for result in [
                latest(body.as_bytes(), None).map(|_| ()),
                window(Endpoint::FiveMinute, body.as_bytes(), None).map(|_| ()),
                timeseries(body.as_bytes(), ItemId(1)).map(|_| ()),
                mapping(body.as_bytes()).map(|_| ()),
            ] {
                if let Err(error) = result {
                    assert!(error.to_string().len() < 200, "{}", error.to_string().len());
                    assert!(!error.to_string().contains("xxxx"));
                    assert!(!format!("{error:?}").contains("xxxx"));
                }
            }
        }
        let overlong_average = format!(
            r#"{{"data":{{"1":{{"avgHighPrice":1.{},"highPriceVolume":1,"avgLowPrice":null,"lowPriceVolume":0}}}}}}"#,
            "9".repeat(1_000_000)
        );
        let error = window(Endpoint::FiveMinute, overlong_average.as_bytes(), None).unwrap_err();
        assert!(error.to_string().len() < 200);
        assert!(!error.to_string().contains("999"));
    }
}
