//! Typed results of Hardcore Grand Exchange queries.
//!
//! Units are explicit in the types: [`Price`] is whole GP, [`AveragePrice`] is
//! hundredths of a GP (the API reports averages with up to two decimals), and
//! [`UnixSeconds`] is seconds since the Unix epoch. Absent values are
//! `Option`, never zero.

use std::collections::BTreeMap;
use std::time::Duration;

/// An item identifier as used by the prices API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u32);

/// Seconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixSeconds(pub u64);

/// A whole-GP price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub u64);

/// An average price in hundredths of a GP, so `227.22` is `AveragePrice(22722)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AveragePrice {
    /// The average in hundredths of a GP.
    pub hundredths: u64,
}

/// The world type a result describes. There is no default and no fallback to
/// another world type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorldType {
    /// Hardcore Worlds, which have their own Grand Exchange economy.
    Hardcore,
}

/// Which API endpoint produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// `/mapping`: item metadata.
    Mapping,
    /// `/latest`: latest observed trades.
    Latest,
    /// `/5m`: five-minute averages and volumes.
    FiveMinute,
    /// `/1h`: one-hour averages and volumes.
    OneHour,
    /// `/timeseries`: averages and volumes over a lookback.
    Timeseries,
}

impl Endpoint {
    /// The path segment appended to the world's base URL.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Mapping => "mapping",
            Self::Latest => "latest",
            Self::FiveMinute => "5m",
            Self::OneHour => "1h",
            Self::Timeseries => "timeseries",
        }
    }
}

/// Where and when a value was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    /// The world type the data describes.
    pub world_type: WorldType,
    /// The endpoint that returned it.
    pub endpoint: Endpoint,
    /// When this process received the response. This is not the time of the
    /// trades inside it; those are carried by the values themselves.
    pub retrieved_at: UnixSeconds,
}

/// A value together with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed<T> {
    /// The parsed value.
    pub value: T,
    /// Where and when it came from.
    pub provenance: Provenance,
}

/// A price and the time its trade was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedPrice {
    /// The traded price.
    pub price: Price,
    /// When the trade happened.
    pub observed_at: UnixSeconds,
}

/// Latest observed prices for one item. A side that has never been observed is
/// `None`; it is not zero and not the other side's price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatestPrices {
    /// Latest instant-buy price (the API's `high`).
    pub high: Option<TimedPrice>,
    /// Latest instant-sell price (the API's `low`).
    pub low: Option<TimedPrice>,
}

/// Average price and trade count for one side over a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideStats {
    /// Average price, or `None` when nothing traded on this side.
    pub average_price: Option<AveragePrice>,
    /// Number of trades on this side in the window.
    pub volume: u64,
}

/// Both sides' statistics over one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowStats {
    /// Instant-buy side (`avgHighPrice`, `highPriceVolume`).
    pub high: SideStats,
    /// Instant-sell side (`avgLowPrice`, `lowPriceVolume`).
    pub low: SideStats,
}

/// The length of an aggregation window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLength {
    /// Five minutes.
    FiveMinutes,
    /// One hour.
    OneHour,
}

impl WindowLength {
    pub(crate) const fn endpoint(self) -> Endpoint {
        match self {
            Self::FiveMinutes => Endpoint::FiveMinute,
            Self::OneHour => Endpoint::OneHour,
        }
    }
}

/// All items' statistics for one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceWindow {
    /// Start of the window as reported by the API, if it reported one.
    pub starts_at: Option<UnixSeconds>,
    /// Items that traded or were listed in the window.
    pub items: BTreeMap<ItemId, WindowStats>,
}

/// One point of a time series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeriesPoint {
    /// Start of the step.
    pub timestamp: UnixSeconds,
    /// Statistics for the step.
    pub stats: WindowStats,
}

/// A price history for one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The item the series describes.
    pub item: ItemId,
    /// Start of the lookback.
    pub starts_at: UnixSeconds,
    /// End of the lookback.
    pub ends_at: UnixSeconds,
    /// Seconds per step. The API does not guarantee this stays constant.
    pub timestep_seconds: u64,
    /// Points in the order returned.
    pub points: Vec<SeriesPoint>,
}

/// How far back a time series looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookback {
    /// Six hours.
    SixHours,
    /// Twenty-four hours.
    TwentyFourHours,
    /// Seven days.
    SevenDays,
    /// Thirty days.
    ThirtyDays,
    /// Six months.
    SixMonths,
    /// One year.
    OneYear,
}

impl Lookback {
    /// The query value the API expects.
    #[must_use]
    pub const fn as_query(self) -> &'static str {
        match self {
            Self::SixHours => "6h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
            Self::ThirtyDays => "30d",
            Self::SixMonths => "6m",
            Self::OneYear => "1y",
        }
    }
}

/// Item metadata. Only `id` and `name` are required; the other fields are
/// optional because their presence for every item has not been verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    /// The item id.
    pub id: ItemId,
    /// The item name. Untrusted text from the API.
    pub name: String,
    /// Whether the item is members-only.
    pub members: Option<bool>,
    /// Grand Exchange buy limit.
    pub buy_limit: Option<u32>,
    /// Store value in GP.
    pub value: Option<u64>,
    /// Low alchemy value in GP.
    pub low_alchemy: Option<u64>,
    /// High alchemy value in GP.
    pub high_alchemy: Option<u64>,
}

/// Whether an observation is recent enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Age is within the limit. An observation dated after `now` (clock skew)
    /// counts as age zero.
    Fresh,
    /// Older than the limit.
    Stale {
        /// Age in whole seconds.
        age_seconds: u64,
    },
}

/// Classifies an observation's age against an explicit limit. Pure: the caller
/// supplies `now`.
#[must_use]
pub const fn classify_age(
    observed_at: UnixSeconds,
    now: UnixSeconds,
    max_age: Duration,
) -> Freshness {
    let age_seconds = now.0.saturating_sub(observed_at.0);
    if age_seconds > max_age.as_secs() {
        Freshness::Stale { age_seconds }
    } else {
        Freshness::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn age_at_the_limit_is_fresh_and_one_second_past_is_stale() {
        let observed = UnixSeconds(1_000);
        assert_eq!(
            classify_age(observed, UnixSeconds(1_000 + 3600), HOUR),
            Freshness::Fresh
        );
        assert_eq!(
            classify_age(observed, UnixSeconds(1_000 + 3601), HOUR),
            Freshness::Stale { age_seconds: 3601 }
        );
    }

    #[test]
    fn observation_from_the_future_counts_as_fresh() {
        assert_eq!(
            classify_age(UnixSeconds(2_000), UnixSeconds(1_000), HOUR),
            Freshness::Fresh
        );
    }

    #[test]
    fn lookback_and_endpoint_names_match_the_api() {
        assert_eq!(Lookback::TwentyFourHours.as_query(), "24h");
        assert_eq!(Lookback::SixMonths.as_query(), "6m");
        assert_eq!(WindowLength::FiveMinutes.endpoint().path(), "5m");
        assert_eq!(WindowLength::OneHour.endpoint().path(), "1h");
    }
}
