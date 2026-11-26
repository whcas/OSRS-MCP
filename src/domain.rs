//! Value types shared by the economy, market and account modules.
//!
//! This module holds only concepts that more than one part of the crate needs
//! to agree on. It has no I/O, no clock and no dependencies, and knows nothing
//! about MCP, HTTP or any particular data source. In particular it carries no
//! provenance, world type or freshness: those describe an *observation* and
//! belong to the layer that obtains the observation. Pure calculations such as
//! [`crate::profit`] take these plain values, and analysis code that combines
//! observations with calculations attaches provenance around them.
//!
//! # Money: three deliberately different types
//!
//! | Type | Meaning | Range |
//! |---|---|---|
//! | [`Price`] | A quoted or traded price of one item | `0..=u64::MAX` GP |
//! | [`AveragePrice`] | An average of prices, which can be fractional | hundredths of a GP |
//! | [`Gp`] | A signed amount of coins: profit, loss, cost, fee | `i64` GP |
//!
//! They are not interchangeable, and there are no implicit conversions:
//!
//! - A `Price` becomes a `Gp` with [`TryFrom`], which fails only above
//!   `i64::MAX`. A `Gp` becomes a `Price` with [`TryFrom`], which fails if it
//!   is negative.
//! - An [`AveragePrice`] has no conversion to a whole-GP type. Whether to
//!   round, and in which direction, is a decision the analysis makes with a
//!   reason (for example, pessimistic for the buy side), so it is not hidden
//!   in this module.
//!
//! # Counts
//!
//! A [`Quantity`] is strictly positive: it is the number of items in a trade or
//! recipe line. Trade *volumes* observed in a market window can legitimately be
//! zero and are plain `u64` counts, not `Quantity`.
//!
//! # Identifiers and time
//!
//! [`ItemId`] is the game's item identifier. [`UnixSeconds`] is whole seconds
//! since the Unix epoch, UTC, and is never negative; importers holding signed
//! or higher-precision timestamps keep their own representation and convert
//! with [`TryFrom`].

use std::fmt;

/// A signed amount of coins (GP), such as a profit, loss, cost or fee.
///
/// Signed so that losses are representable. For the price of one item use
/// [`Price`], which cannot be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Gp(i64);

impl Gp {
    /// Wraps a whole number of coins.
    #[must_use]
    pub const fn new(amount: i64) -> Self {
        Self(amount)
    }

    /// Returns the amount in coins.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Adds two amounts, or `None` if the sum does not fit.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(sum) => Some(Self(sum)),
            None => None,
        }
    }

    /// Subtracts `other`, or `None` if the difference does not fit.
    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(difference) => Some(Self(difference)),
            None => None,
        }
    }

    /// Multiplies by a quantity, or `None` if the product does not fit.
    #[must_use]
    pub fn checked_mul_quantity(self, quantity: Quantity) -> Option<Self> {
        let count = i64::try_from(quantity.get()).ok()?;
        self.0.checked_mul(count).map(Self)
    }
}

/// A non-negative price in whole GP for a single item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub u64);

/// An average price in hundredths of a GP, so `227.22` is `22_722`.
///
/// Exact decimal values are kept as integers rather than floating point so
/// that the stored value is what the source reported. There is deliberately no
/// conversion to [`Price`] or [`Gp`]; see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AveragePrice {
    /// The average in hundredths of a GP.
    pub hundredths: u64,
}

/// A strictly positive item count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quantity(u64);

impl Quantity {
    /// Creates a quantity, rejecting zero.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ZeroQuantity`] if `count` is zero.
    pub const fn new(count: u64) -> Result<Self, DomainError> {
        if count == 0 {
            Err(DomainError::ZeroQuantity)
        } else {
            Ok(Self(count))
        }
    }

    /// Returns the count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An item identifier as used by the game and the prices API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u32);

/// Whole seconds since the Unix epoch, UTC. Never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixSeconds(pub u64);

/// Why a domain value could not be created or converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    /// A [`Quantity`] must be at least one.
    ZeroQuantity,
    /// The value was negative where only a non-negative one is allowed.
    NegativeValue,
    /// The value is too large for the target type.
    OutOfRange,
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroQuantity => "quantity must be at least one",
            Self::NegativeValue => "value must not be negative",
            Self::OutOfRange => "value is too large for the target type",
        })
    }
}

impl std::error::Error for DomainError {}

impl TryFrom<Price> for Gp {
    type Error = DomainError;

    fn try_from(price: Price) -> Result<Self, Self::Error> {
        i64::try_from(price.0)
            .map(Self)
            .map_err(|_| DomainError::OutOfRange)
    }
}

impl TryFrom<Gp> for Price {
    type Error = DomainError;

    fn try_from(amount: Gp) -> Result<Self, Self::Error> {
        u64::try_from(amount.0)
            .map(Self)
            .map_err(|_| DomainError::NegativeValue)
    }
}

impl TryFrom<i64> for UnixSeconds {
    type Error = DomainError;

    fn try_from(seconds: i64) -> Result<Self, Self::Error> {
        u64::try_from(seconds)
            .map(Self)
            .map_err(|_| DomainError::NegativeValue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qty(count: u64) -> Quantity {
        Quantity::new(count).unwrap()
    }

    #[test]
    fn quantity_rejects_zero_and_accepts_positive_counts() {
        assert_eq!(Quantity::new(0), Err(DomainError::ZeroQuantity));
        assert_eq!(Quantity::new(1).map(Quantity::get), Ok(1));
        assert_eq!(Quantity::new(u64::MAX).map(Quantity::get), Ok(u64::MAX));
    }

    #[test]
    fn gp_is_signed() {
        assert_eq!(Gp::new(-5).get(), -5);
        assert!(Gp::new(-1) < Gp::new(0));
    }

    #[test]
    fn gp_checked_arithmetic_reports_overflow_instead_of_wrapping() {
        let max = Gp::new(i64::MAX);
        let min = Gp::new(i64::MIN);
        assert_eq!(Gp::new(2).checked_add(Gp::new(3)), Some(Gp::new(5)));
        assert_eq!(max.checked_add(Gp::new(1)), None);
        assert_eq!(Gp::new(2).checked_sub(Gp::new(5)), Some(Gp::new(-3)));
        assert_eq!(min.checked_sub(Gp::new(1)), None);
        assert_eq!(max.checked_sub(Gp::new(-1)), None);
    }

    #[test]
    fn gp_checked_multiplication_by_quantity() {
        assert_eq!(Gp::new(7).checked_mul_quantity(qty(3)), Some(Gp::new(21)));
        assert_eq!(Gp::new(-7).checked_mul_quantity(qty(3)), Some(Gp::new(-21)));
        assert_eq!(Gp::new(2).checked_mul_quantity(qty(i64::MAX as u64)), None);
        // A quantity that does not fit in i64 cannot be applied at all, even
        // to a zero amount, so the failure never depends on the price.
        assert_eq!(Gp::new(0).checked_mul_quantity(qty(u64::MAX)), None);
        assert_eq!(
            Gp::new(1).checked_mul_quantity(qty(i64::MAX as u64)),
            Some(Gp::new(i64::MAX))
        );
    }

    #[test]
    fn price_converts_to_gp_unless_it_exceeds_i64() {
        assert_eq!(Gp::try_from(Price(0)), Ok(Gp::new(0)));
        assert_eq!(Gp::try_from(Price(i64::MAX as u64)), Ok(Gp::new(i64::MAX)));
        assert_eq!(
            Gp::try_from(Price(i64::MAX as u64 + 1)),
            Err(DomainError::OutOfRange)
        );
    }

    #[test]
    fn gp_converts_to_price_unless_negative() {
        assert_eq!(Price::try_from(Gp::new(0)), Ok(Price(0)));
        assert_eq!(
            Price::try_from(Gp::new(i64::MAX)),
            Ok(Price(i64::MAX as u64))
        );
        assert_eq!(
            Price::try_from(Gp::new(-1)),
            Err(DomainError::NegativeValue)
        );
        assert_eq!(
            Price::try_from(Gp::new(i64::MIN)),
            Err(DomainError::NegativeValue)
        );
    }

    #[test]
    fn unix_seconds_rejects_negative_values() {
        assert_eq!(UnixSeconds::try_from(0_i64), Ok(UnixSeconds(0)));
        assert_eq!(
            UnixSeconds::try_from(1_794_623_592_i64),
            Ok(UnixSeconds(1_794_623_592))
        );
        assert_eq!(
            UnixSeconds::try_from(-1_i64),
            Err(DomainError::NegativeValue)
        );
    }

    #[test]
    fn average_price_keeps_the_exact_reported_decimal() {
        // 227.22 GP is stored as 22,722 hundredths, with no floating point.
        let average = AveragePrice { hundredths: 22_722 };
        assert_eq!(average.hundredths, 22_722);
        assert!(average < AveragePrice { hundredths: 22_723 });
    }

    #[test]
    fn identifiers_and_times_order_and_hash_by_value() {
        use std::collections::BTreeSet;
        let items: BTreeSet<_> = [ItemId(4151), ItemId(2), ItemId(4151)].into();
        assert_eq!(
            items.into_iter().collect::<Vec<_>>(),
            [ItemId(2), ItemId(4151)]
        );
        assert!(UnixSeconds(1) < UnixSeconds(2));
    }

    #[test]
    fn errors_have_readable_messages() {
        for error in [
            DomainError::ZeroQuantity,
            DomainError::NegativeValue,
            DomainError::OutOfRange,
        ] {
            assert_ne!(error.to_string(), "");
        }
    }
}
