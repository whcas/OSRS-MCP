//! Pure assessment of the latest observed prices for one item.
//!
//! The assessment keeps every distinction the source makes: an item the
//! response omitted, a side that returned no observation, a real price of zero,
//! crossed sides, and an observation that is stale or dated in the future. Age
//! is always measured against a caller-supplied `now`, normally the retrieval
//! time of the response, never the current clock.

use std::time::Duration;

use thiserror::Error;

use crate::domain::{Price, UnixSeconds};

use super::model::{Freshness, LatestPrices, TimedPrice, classify_age};

/// Age limit used when the caller supplies none: one hour.
pub const DEFAULT_MAX_AGE_SECONDS: u64 = 3_600;
/// Largest age limit a caller may supply: 30 days.
pub const MAX_AGE_SECONDS: u64 = 2_592_000;

/// The age limit was above [`MAX_AGE_SECONDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("`max_age_seconds` must be at most {MAX_AGE_SECONDS}")]
pub struct MaxAgeError;

/// How old an observation may be and still count as fresh, inclusive. Zero is
/// valid and means only an observation made exactly at the reference time is
/// fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxAge(u64);

impl MaxAge {
    /// The default limit of [`DEFAULT_MAX_AGE_SECONDS`].
    pub const DEFAULT: Self = Self(DEFAULT_MAX_AGE_SECONDS);

    /// Creates a limit of `seconds`.
    ///
    /// # Errors
    ///
    /// Returns [`MaxAgeError`] above [`MAX_AGE_SECONDS`].
    pub const fn new(seconds: u64) -> Result<Self, MaxAgeError> {
        if seconds > MAX_AGE_SECONDS {
            Err(MaxAgeError)
        } else {
            Ok(Self(seconds))
        }
    }

    /// The limit in whole seconds.
    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.0
    }
}

/// An observation's age relative to the reference time, with its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationAge {
    /// Within the limit; the age is in whole seconds.
    Fresh {
        /// Age in whole seconds.
        age_seconds: u64,
    },
    /// Older than the limit; the age is in whole seconds.
    Stale {
        /// Age in whole seconds.
        age_seconds: u64,
    },
    /// Dated after the reference time, which indicates clock skew or bad data.
    Future {
        /// How far ahead of the reference time, in whole seconds.
        ahead_seconds: u64,
    },
}

/// One side's price with when it was observed and how old that is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideAssessment {
    /// The observed price in whole GP. May legitimately be zero.
    pub price: Price,
    /// When the trade was observed.
    pub observed_at: UnixSeconds,
    /// Age and freshness against the reference time.
    pub age: ObservationAge,
}

/// The latest observations for one item, assessed against a reference time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatestAssessment {
    /// Whether the item appeared in the response at all. Presence says nothing
    /// about whether the item can be traded.
    pub present_in_latest: bool,
    /// The last instant-buy observation, or `None` if none was returned.
    pub high: Option<SideAssessment>,
    /// The last instant-sell observation, or `None` if none was returned.
    pub low: Option<SideAssessment>,
    /// For an item with both sides: whether the instant-sell price is above the
    /// instant-buy price. Both prices are kept as observed. `None` unless both
    /// sides were returned.
    pub crossed: Option<bool>,
}

/// Assesses `entry`, an item's entry in a latest response, or `None` if the
/// response omitted the item. Pure: the caller supplies `now` and the limit.
#[must_use]
pub fn assess_latest(
    entry: Option<&LatestPrices>,
    now: UnixSeconds,
    max_age: MaxAge,
) -> LatestAssessment {
    let side = |timed: Option<TimedPrice>| timed.map(|timed| assess_side(timed, now, max_age));
    let high = entry.and_then(|prices| side(prices.high));
    let low = entry.and_then(|prices| side(prices.low));
    let crossed = high.zip(low).map(|(high, low)| low.price > high.price);
    LatestAssessment {
        present_in_latest: entry.is_some(),
        high,
        low,
        crossed,
    }
}

const fn assess_side(timed: TimedPrice, now: UnixSeconds, max_age: MaxAge) -> SideAssessment {
    let age = match classify_age(timed.observed_at, now, Duration::from_secs(max_age.0)) {
        Freshness::Fresh => ObservationAge::Fresh {
            // Not in the future, so this difference is the exact age.
            age_seconds: now.0.abs_diff(timed.observed_at.0),
        },
        Freshness::Stale { age_seconds } => ObservationAge::Stale { age_seconds },
        Freshness::Future { ahead_seconds } => ObservationAge::Future { ahead_seconds },
    };
    SideAssessment {
        price: timed.price,
        observed_at: timed.observed_at,
        age,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: UnixSeconds = UnixSeconds(10_000);

    fn timed(price: u64, observed_at: u64) -> TimedPrice {
        TimedPrice {
            price: Price(price),
            observed_at: UnixSeconds(observed_at),
        }
    }

    fn prices(high: Option<TimedPrice>, low: Option<TimedPrice>) -> LatestPrices {
        LatestPrices { high, low }
    }

    fn assess(entry: Option<&LatestPrices>) -> LatestAssessment {
        assess_latest(entry, NOW, MaxAge::DEFAULT)
    }

    #[test]
    fn an_item_the_response_omitted_is_not_present_and_has_no_sides() {
        let assessment = assess(None);

        assert!(!assessment.present_in_latest);
        assert_eq!(assessment.high, None);
        assert_eq!(assessment.low, None);
        assert_eq!(assessment.crossed, None);
    }

    #[test]
    fn a_present_item_with_no_observations_differs_from_an_omitted_one() {
        let assessment = assess(Some(&prices(None, None)));

        assert!(assessment.present_in_latest);
        assert_eq!(assessment.high, None);
        assert_eq!(assessment.low, None);
        assert_eq!(assessment.crossed, None);
    }

    #[test]
    fn a_missing_side_stays_missing_and_is_never_filled_from_the_other() {
        let assessment = assess(Some(&prices(Some(timed(100, 9_990)), None)));

        assert!(assessment.high.is_some());
        assert_eq!(assessment.low, None);
        assert_eq!(assessment.crossed, None, "crossing needs both sides");
    }

    #[test]
    fn a_real_zero_price_is_kept_as_zero() {
        let assessment = assess(Some(&prices(None, Some(timed(0, 9_990)))));

        assert_eq!(assessment.low.map(|side| side.price), Some(Price(0)));
    }

    #[test]
    fn each_side_is_classified_independently() {
        let assessment = assess(Some(&prices(
            Some(timed(100, 9_990)),
            Some(timed(90, 1_000)),
        )));

        assert_eq!(
            assessment.high.map(|side| side.age),
            Some(ObservationAge::Fresh { age_seconds: 10 })
        );
        assert_eq!(
            assessment.low.map(|side| side.age),
            Some(ObservationAge::Stale { age_seconds: 9_000 })
        );
    }

    #[test]
    fn the_age_limit_is_inclusive_and_zero_is_a_valid_strict_limit() {
        let at_limit = Some(timed(1, 10_000 - DEFAULT_MAX_AGE_SECONDS));
        let past_limit = Some(timed(1, 10_000 - DEFAULT_MAX_AGE_SECONDS - 1));

        let fresh = assess_latest(Some(&prices(at_limit, past_limit)), NOW, MaxAge::DEFAULT);
        assert_eq!(
            fresh.high.map(|side| side.age),
            Some(ObservationAge::Fresh { age_seconds: 3_600 })
        );
        assert_eq!(
            fresh.low.map(|side| side.age),
            Some(ObservationAge::Stale { age_seconds: 3_601 })
        );

        let strict = MaxAge::new(0).unwrap();
        let exact = assess_latest(
            Some(&prices(Some(timed(1, 10_000)), Some(timed(1, 9_999)))),
            NOW,
            strict,
        );
        assert_eq!(
            exact.high.map(|side| side.age),
            Some(ObservationAge::Fresh { age_seconds: 0 })
        );
        assert_eq!(
            exact.low.map(|side| side.age),
            Some(ObservationAge::Stale { age_seconds: 1 })
        );
    }

    #[test]
    fn a_future_dated_observation_reports_how_far_ahead_it_is_never_fresh() {
        let assessment = assess(Some(&prices(Some(timed(100, 10_500)), None)));

        assert_eq!(
            assessment.high.map(|side| side.age),
            Some(ObservationAge::Future { ahead_seconds: 500 })
        );
    }

    #[test]
    fn crossed_sides_keep_both_prices_and_are_flagged() {
        // Instant-sell above instant-buy.
        let assessment = assess(Some(&prices(
            Some(timed(100, 9_990)),
            Some(timed(120, 9_990)),
        )));

        assert_eq!(assessment.crossed, Some(true));
        assert_eq!(assessment.high.map(|side| side.price), Some(Price(100)));
        assert_eq!(assessment.low.map(|side| side.price), Some(Price(120)));
    }

    #[test]
    fn equal_and_ordinary_sides_are_not_crossed() {
        let equal = assess(Some(&prices(
            Some(timed(100, 9_990)),
            Some(timed(100, 9_990)),
        )));
        let ordinary = assess(Some(&prices(
            Some(timed(100, 9_990)),
            Some(timed(80, 9_990)),
        )));

        assert_eq!(equal.crossed, Some(false));
        assert_eq!(ordinary.crossed, Some(false));
    }

    #[test]
    fn the_age_limit_is_bounded() {
        assert!(MaxAge::new(MAX_AGE_SECONDS).is_ok());
        assert_eq!(MaxAge::new(MAX_AGE_SECONDS + 1), Err(MaxAgeError));
        assert_eq!(MaxAge::DEFAULT.seconds(), 3_600);
    }
}
