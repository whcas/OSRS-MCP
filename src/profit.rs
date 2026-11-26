//! Deterministic profit primitives for trades and processing recipes.
//!
//! Everything here is a pure function of explicit inputs: no network, storage,
//! clock or MCP types. Callers supply prices, quantities and a [`FeePolicy`];
//! this module only does checked integer arithmetic on them.
//!
//! # Units and arithmetic
//!
//! - Amounts are [`Gp`], a signed whole number of coins (`i64`), so that losses
//!   are representable. Unit prices and fee caps are [`Price`], which cannot be
//!   negative; a price above `i64::MAX` is reported as [`ProfitError::Overflow`]
//!   because it cannot be summed as an amount.
//! - Fee rates are [`FeeRate`], a percentage between 0% and 100% in basis
//!   points, validated when it is created. It is a different type from the
//!   signed return on investment, [`BasisPoints`], which can be negative and
//!   exceed 100%.
//! - Quantities are [`Quantity`], a strictly positive count of items.
//! - All arithmetic is checked. Any intermediate that does not fit in `i64`
//!   yields [`ProfitError::Overflow`]; nothing wraps or saturates.
//!
//! # Rounding
//!
//! - Percentage fees are computed **per unit sold**, rounded **down** (floor)
//!   to whole GP, optionally capped per unit, then multiplied by the quantity.
//!   Whether a given game mechanic rounds this way is an input assumption of
//!   the chosen [`FeePolicy`], not something this module decides.
//! - Return on investment is reported in basis points (1 bp = 0.01%) and
//!   truncated toward zero.
//!
//! # Hardcore worlds
//!
//! Callers pricing Hardcore Main World trades pass [`FeePolicy::NoFees`], an
//! explicit assertion rather than a default. Provenance of that rule:
//!
//! - **Maintainer-verified in game:** the Grand Exchange's Convenience Fees
//!   interface on Hardcore Worlds states that no fee is charged (issue #14,
//!   maintainer observation of 2026-11-26). Not independently verified from
//!   public documentation.
//! - **Standard worlds, for contrast:** the OSRS Wiki's Grand Exchange page,
//!   <https://oldschool.runescape.wiki/w/Grand_Exchange>, documents a 2% tax,
//!   rounded down per item, capped at 5,000,000 GP per item, with some item
//!   exemptions. That is [`FeePolicy::PercentOfSale`] with a rate of 200 basis
//!   points and `cap_per_unit: Some(Price(5_000_000))`, but the item exemptions
//!   are not modelled (see "Scope of a fee policy").
//!
//! This module deliberately contains no world-specific rules, so it stays
//! correct if either rule changes. Standard-world tax is never applied
//! implicitly to Hardcore trades, and vice versa.
//!
//! # Scope of a fee policy
//!
//! One [`FeePolicy`] applies to every output line of a [`Transaction`]. Outputs
//! with different fee treatment (some exempt, some taxed) are not expressible
//! in a single transaction: split them into separate transactions and combine
//! the results in the caller.
//!
//! # Missing data versus zero
//!
//! - A missing unit price is [`ProfitError::MissingPrice`], never treated as
//!   zero. Valuing an input at zero is a decision the caller makes with an
//!   explicit price of `Gp::new(0)`. This module does not infer it: an item the
//!   player already owns still has an opportunity cost (what it could be sold
//!   for), which differs from cash spent, and which one to use is the caller's
//!   choice.
//! - An unknown fee policy ([`FeePolicy::Unknown`]) leaves fees, net profit and
//!   ROI as `None` in the [`Outcome`], while gross figures remain available.
//!   [`FeePolicy::NoFees`] is a deliberate assertion that no fee applies.
//! - ROI is `None` when the acquisition cost is zero, since it is undefined
//!   rather than zero.

use crate::domain::{Gp, Price, Quantity};

/// Basis points in one whole: 10,000 bp is 100%.
const BASIS_POINTS_PER_UNIT: u32 = 10_000;

/// One line of a transaction: a quantity of items at a unit price.
///
/// The price is optional so that a missing observation stays distinguishable
/// from a price of zero; evaluation fails on a missing price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    unit_price: Option<Price>,
    quantity: Quantity,
}

impl Line {
    /// A line with a known unit price.
    #[must_use]
    pub const fn priced(unit_price: Price, quantity: Quantity) -> Self {
        Self {
            unit_price: Some(unit_price),
            quantity,
        }
    }

    /// A line whose unit price is not known.
    #[must_use]
    pub const fn unpriced(quantity: Quantity) -> Self {
        Self {
            unit_price: None,
            quantity,
        }
    }
}

/// How fees on sold items are determined. There is no default: the caller
/// must say, because Hardcore-world fee rules are not assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeePolicy {
    /// The fee rules are not known. Fees, net profit and ROI are left unknown.
    Unknown,
    /// Asserted: selling incurs no fee.
    NoFees,
    /// A percentage of each unit's sale price.
    PercentOfSale {
        /// The share of the sale price taken as a fee.
        rate: FeeRate,
        /// Optional ceiling on the fee for a single unit, applied after
        /// rounding down. A cap too large to represent as [`Gp`] cannot bind.
        cap_per_unit: Option<Price>,
    },
}

/// A fee rate of 0% to 100% inclusive, held in basis points (1 bp = 0.01%).
///
/// The bound is checked at construction, so a rate above 100% cannot reach the
/// fee arithmetic. This is not [`BasisPoints`]: that type is a signed return on
/// investment and may be negative or exceed 100%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeeRate(u32);

impl FeeRate {
    /// Creates a rate from basis points, for example `200` for 2%.
    ///
    /// # Errors
    ///
    /// Returns [`FeeRateError::AboveFullRate`] above 10,000 basis points.
    pub const fn from_basis_points(basis_points: u32) -> Result<Self, FeeRateError> {
        if basis_points > BASIS_POINTS_PER_UNIT {
            Err(FeeRateError::AboveFullRate)
        } else {
            Ok(Self(basis_points))
        }
    }

    /// Returns the rate in basis points, between 0 and 10,000 inclusive.
    #[must_use]
    pub const fn basis_points(self) -> u32 {
        self.0
    }
}

/// Why a [`FeeRate`] could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FeeRateError {
    /// The rate exceeded 100%.
    #[error("fee rate must be at most 10000 basis points")]
    AboveFullRate,
}

/// A set of items consumed and a set produced and sold, such as a trade
/// (one input, one output) or a processing recipe (several of each).
///
/// Fees apply to outputs only. Inputs are priced as acquired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    inputs: Vec<Line>,
    outputs: Vec<Line>,
    fees: FeePolicy,
}

impl Transaction {
    /// A flip: buy `quantity` at `buy_price`, sell the same quantity at
    /// `sell_price`.
    #[must_use]
    pub fn trade(
        buy_price: Option<Price>,
        sell_price: Option<Price>,
        quantity: Quantity,
        fees: FeePolicy,
    ) -> Self {
        let line = |price| Line {
            unit_price: price,
            quantity,
        };
        Self {
            inputs: vec![line(buy_price)],
            outputs: vec![line(sell_price)],
            fees,
        }
    }

    /// A processing recipe with explicit input and output lines.
    #[must_use]
    pub const fn recipe(inputs: Vec<Line>, outputs: Vec<Line>, fees: FeePolicy) -> Self {
        Self {
            inputs,
            outputs,
            fees,
        }
    }
}

/// Return on investment in basis points (100 bp = 1%), truncated toward zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BasisPoints(i128);

impl BasisPoints {
    /// Returns the value in basis points.
    #[must_use]
    pub const fn get(self) -> i128 {
        self.0
    }
}

/// The evaluated result of a [`Transaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Total paid for all inputs.
    pub acquisition_cost: Gp,
    /// Total value of all outputs before fees.
    pub gross_revenue: Gp,
    /// `gross_revenue - acquisition_cost`; may be negative.
    pub gross_profit: Gp,
    /// Total fees, or `None` if the fee policy is unknown.
    pub fees: Option<Gp>,
    /// `gross_profit - fees`, or `None` if the fee policy is unknown.
    pub net_profit: Option<Gp>,
    /// `net_profit / acquisition_cost`, or `None` if net profit is unknown or
    /// the cost is zero.
    pub roi: Option<BasisPoints>,
}

/// Why a transaction could not be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProfitError {
    /// A unit price was missing (as opposed to zero).
    #[error("unit price is missing")]
    MissingPrice,
    /// The transaction has no input lines.
    #[error("transaction has no inputs")]
    NoInputs,
    /// The transaction has no output lines.
    #[error("transaction has no outputs")]
    NoOutputs,
    /// An intermediate value did not fit in a signed 64-bit amount.
    #[error("amount overflows a signed 64-bit GP value")]
    Overflow,
}

/// Evaluates a transaction.
///
/// # Errors
///
/// Returns a [`ProfitError`] for missing prices, empty input or output sets, or
/// arithmetic overflow (including a price above `i64::MAX`).
pub fn evaluate(transaction: &Transaction) -> Result<Outcome, ProfitError> {
    if transaction.inputs.is_empty() {
        return Err(ProfitError::NoInputs);
    }
    if transaction.outputs.is_empty() {
        return Err(ProfitError::NoOutputs);
    }

    let acquisition_cost = total_value(&transaction.inputs)?;
    let gross_revenue = total_value(&transaction.outputs)?;
    let gross_profit = subtract(gross_revenue, acquisition_cost)?;

    let fees = total_fees(&transaction.outputs, transaction.fees)?;
    let net_profit = fees.map(|fees| subtract(gross_profit, fees)).transpose()?;
    let roi = net_profit.and_then(|net| return_on_investment(net, acquisition_cost));

    Ok(Outcome {
        acquisition_cost,
        gross_revenue,
        gross_profit,
        fees,
        net_profit,
        roi,
    })
}

/// A line's unit price as a signed amount, rejecting a missing price.
fn unit_price(line: &Line) -> Result<Gp, ProfitError> {
    let price = line.unit_price.ok_or(ProfitError::MissingPrice)?;
    Gp::try_from(price).map_err(|_| ProfitError::Overflow)
}

fn add(a: Gp, b: Gp) -> Result<Gp, ProfitError> {
    a.checked_add(b).ok_or(ProfitError::Overflow)
}

fn subtract(a: Gp, b: Gp) -> Result<Gp, ProfitError> {
    a.checked_sub(b).ok_or(ProfitError::Overflow)
}

fn times_quantity(amount: Gp, quantity: Quantity) -> Result<Gp, ProfitError> {
    amount
        .checked_mul_quantity(quantity)
        .ok_or(ProfitError::Overflow)
}

fn total_value(lines: &[Line]) -> Result<Gp, ProfitError> {
    lines.iter().try_fold(Gp::new(0), |total, line| {
        add(total, times_quantity(unit_price(line)?, line.quantity)?)
    })
}

/// Total fees on the sold lines; `None` when the policy is unknown.
fn total_fees(sold: &[Line], policy: FeePolicy) -> Result<Option<Gp>, ProfitError> {
    match policy {
        FeePolicy::Unknown => Ok(None),
        FeePolicy::NoFees => Ok(Some(Gp::new(0))),
        FeePolicy::PercentOfSale { rate, cap_per_unit } => sold
            .iter()
            .try_fold(Gp::new(0), |total, line| {
                let per_unit = fee_per_unit(unit_price(line)?, rate, cap_per_unit);
                add(total, times_quantity(per_unit, line.quantity)?)
            })
            .map(Some),
    }
}

/// Floor of `price * rate`, then capped.
///
/// `price` is never negative because it comes from a [`Price`]. Splitting it as
/// `whole * 10_000 + remainder` makes the floor exact without any intermediate
/// that can overflow: `whole * rate` is at most `price`, and the remainder term
/// is below `10_000 * 10_000`. The rate is at most 100%, so the fee never
/// exceeds the price.
fn fee_per_unit(price: Gp, rate: FeeRate, cap: Option<Price>) -> Gp {
    let per_whole = i64::from(BASIS_POINTS_PER_UNIT);
    let rate = i64::from(rate.basis_points());
    let (whole, remainder) = (price.get() / per_whole, price.get() % per_whole);
    let fee = whole * rate + remainder * rate / per_whole;
    // A cap that does not fit in `Gp` is larger than any possible fee.
    let cap = cap.and_then(|cap| Gp::try_from(cap).ok());
    Gp::new(cap.map_or(fee, |cap| fee.min(cap.get())))
}

fn return_on_investment(net_profit: Gp, cost: Gp) -> Option<BasisPoints> {
    if cost.get() == 0 {
        return None;
    }
    let scaled = i128::from(net_profit.get()) * i128::from(BASIS_POINTS_PER_UNIT);
    Some(BasisPoints(scaled / i128::from(cost.get())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gp(amount: i64) -> Gp {
        Gp::new(amount)
    }

    fn qty(count: u64) -> Quantity {
        Quantity::new(count).unwrap()
    }

    fn rate(basis_points: u32) -> FeeRate {
        FeeRate::from_basis_points(basis_points).unwrap()
    }

    fn percent_of_sale(basis_points: u32, cap_per_unit: Option<u64>) -> FeePolicy {
        FeePolicy::PercentOfSale {
            rate: rate(basis_points),
            cap_per_unit: cap_per_unit.map(Price),
        }
    }

    /// An illustrative 2% policy used to exercise the arithmetic. It is a test
    /// input only, not a claim about any world's fee rules.
    fn two_percent() -> FeePolicy {
        percent_of_sale(200, None)
    }

    fn trade(buy: u64, sell: u64, count: u64, fees: FeePolicy) -> Transaction {
        Transaction::trade(Some(Price(buy)), Some(Price(sell)), qty(count), fees)
    }

    #[test]
    fn worked_trade_with_percentage_fee() {
        // Buy 100 at 1,000 = 100,000. Sell 100 at 1,100 = 110,000.
        // Fee per unit = floor(1,100 * 2%) = 22, so 2,200 in total.
        // Net = 110,000 - 100,000 - 2,200 = 7,800, which is 7.8% = 780 bp.
        let outcome = evaluate(&trade(1_000, 1_100, 100, two_percent())).unwrap();
        assert_eq!(outcome.acquisition_cost, gp(100_000));
        assert_eq!(outcome.gross_revenue, gp(110_000));
        assert_eq!(outcome.gross_profit, gp(10_000));
        assert_eq!(outcome.fees, Some(gp(2_200)));
        assert_eq!(outcome.net_profit, Some(gp(7_800)));
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(780));
    }

    #[test]
    fn worked_hardcore_trade_with_an_explicit_zero_fee_policy() {
        // Maintainer-verified: no GE fee on Hardcore Main Worlds, asserted by the
        // caller with `NoFees`. Buy 100 at 1,000; sell 100 at 1,100.
        // Net = 110,000 - 100,000 = 10,000, which is 10% = 1,000 bp.
        let outcome = evaluate(&trade(1_000, 1_100, 100, FeePolicy::NoFees)).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));
        assert_eq!(outcome.net_profit, Some(gp(10_000)));
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(1_000));
    }

    #[test]
    fn worked_multi_input_recipe() {
        // Inputs: 2 x A at 300, 1 x B at 250, 1 x C valued at 0 by the
        // caller (for example, an item already owned whose opportunity cost the
        // caller chooses to ignore).
        // Cost = 600 + 250 + 0 = 850.
        // Outputs: 3 x D at 400 (1,200) and 1 x E at 99 (99) = 1,299.
        // Fees at 2% floored per unit: D 8 each = 24, E floor(1.98) = 1 -> 25.
        // Net = 1,299 - 850 - 25 = 424. ROI = 424 * 10,000 / 850 = 4,988 bp.
        let recipe = Transaction::recipe(
            vec![
                Line::priced(Price(300), qty(2)),
                Line::priced(Price(250), qty(1)),
                Line::priced(Price(0), qty(1)),
            ],
            vec![
                Line::priced(Price(400), qty(3)),
                Line::priced(Price(99), qty(1)),
            ],
            two_percent(),
        );
        let outcome = evaluate(&recipe).unwrap();
        assert_eq!(outcome.acquisition_cost, gp(850));
        assert_eq!(outcome.gross_revenue, gp(1_299));
        assert_eq!(outcome.fees, Some(gp(25)));
        assert_eq!(outcome.net_profit, Some(gp(424)));
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(4_988));
    }

    #[test]
    fn loss_is_negative() {
        // Buy 3 at 1,000; sell 3 at 900, no fees: net -300 on cost 3,000 = -10%.
        let outcome = evaluate(&trade(1_000, 900, 3, FeePolicy::NoFees)).unwrap();
        assert_eq!(outcome.gross_profit, gp(-300));
        assert_eq!(outcome.net_profit, Some(gp(-300)));
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(-1_000));
    }

    #[test]
    fn roi_truncates_toward_zero_for_non_exact_ratios() {
        // Net +1 on cost 3 is 3,333.33 bp; net -1 on cost 3 is -3,333.33 bp.
        let roi = |sell| evaluate(&trade(3, sell, 1, FeePolicy::NoFees)).unwrap().roi;
        assert_eq!(roi(4).map(BasisPoints::get), Some(3_333));
        assert_eq!(roi(2).map(BasisPoints::get), Some(-3_333));
    }

    #[test]
    fn break_even_has_zero_profit_and_zero_roi() {
        let outcome = evaluate(&trade(500, 500, 10, FeePolicy::NoFees)).unwrap();
        assert_eq!(outcome.net_profit, Some(gp(0)));
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(0));
    }

    #[test]
    fn fee_rounds_down_to_zero_for_low_priced_items() {
        // 2% of 49 is 0.98, floored to 0 per unit.
        let outcome = evaluate(&trade(10, 49, 1_000, two_percent())).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));
    }

    /// The standard-world Grand Exchange rule as documented on the OSRS Wiki:
    /// 2%, rounded down per item, capped at 5,000,000 GP per item. Expressed as
    /// an explicit policy value in tests only; this module hardcodes no world.
    fn standard_world_tax() -> FeePolicy {
        percent_of_sale(200, Some(5_000_000))
    }

    fn fees_for_one_unit_sold_at(price: u64, policy: FeePolicy) -> Gp {
        evaluate(&trade(0, price, 1, policy)).unwrap().fees.unwrap()
    }

    #[test]
    fn rounding_boundary_between_49_and_50_gp() {
        // 2% of 49 is 0.98 and floors to 0; 2% of 50 is exactly 1.
        assert_eq!(fees_for_one_unit_sold_at(49, standard_world_tax()), gp(0));
        assert_eq!(fees_for_one_unit_sold_at(50, standard_world_tax()), gp(1));
        // The next boundary: 2% of 99 is 1.98 -> 1, and of 100 is exactly 2.
        assert_eq!(fees_for_one_unit_sold_at(99, standard_world_tax()), gp(1));
        assert_eq!(fees_for_one_unit_sold_at(100, standard_world_tax()), gp(2));
    }

    #[test]
    fn fee_is_rounded_per_unit_before_multiplying_by_quantity() {
        // 49 GP each: every unit floors to 0, so selling 1,000 of them is
        // untaxed. Rounding the 49,000 total instead would give 980.
        let outcome = evaluate(&trade(0, 49, 1_000, standard_world_tax())).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));

        // 149 GP each: floor(2.98) = 2 per unit, so 3 units pay 6. Rounding
        // the 447 total instead would give floor(8.94) = 8.
        let outcome = evaluate(&trade(0, 149, 3, standard_world_tax())).unwrap();
        assert_eq!(outcome.fees, Some(gp(6)));
    }

    #[test]
    fn five_million_cap_boundary_at_the_per_item_fee() {
        // 2% reaches exactly 5,000,000 at a 250,000,000 sale price.
        assert_eq!(
            fees_for_one_unit_sold_at(249_999_999, standard_world_tax()),
            gp(4_999_999)
        );
        assert_eq!(
            fees_for_one_unit_sold_at(250_000_000, standard_world_tax()),
            gp(5_000_000)
        );
        // Above that the cap binds.
        assert_eq!(
            fees_for_one_unit_sold_at(250_000_001, standard_world_tax()),
            gp(5_000_000)
        );
        assert_eq!(
            fees_for_one_unit_sold_at(2_000_000_000, standard_world_tax()),
            gp(5_000_000)
        );
    }

    #[test]
    fn the_cap_applies_per_item_not_per_sale() {
        // Three items at 500,000,000: each is capped at 5,000,000, so 15,000,000.
        let outcome = evaluate(&trade(0, 500_000_000, 3, standard_world_tax())).unwrap();
        assert_eq!(outcome.fees, Some(gp(15_000_000)));
        assert_eq!(outcome.net_profit, Some(gp(1_485_000_000)));
    }

    #[test]
    fn hardcore_zero_fee_policy_charges_nothing_where_standard_tax_would() {
        // Same high-value sale under both explicit policies: the standard-world
        // policy takes the capped 5,000,000 per item, the Hardcore assertion
        // takes nothing and leaves the whole sale as profit.
        let standard = evaluate(&trade(0, 500_000_000, 2, standard_world_tax())).unwrap();
        assert_eq!(standard.fees, Some(gp(10_000_000)));

        let hardcore = evaluate(&trade(0, 500_000_000, 2, FeePolicy::NoFees)).unwrap();
        assert_eq!(hardcore.fees, Some(gp(0)));
        assert_eq!(hardcore.net_profit, Some(gp(1_000_000_000)));

        // Low-priced items that the standard rule rounds to zero anyway.
        let hardcore = evaluate(&trade(0, 50, 7, FeePolicy::NoFees)).unwrap();
        assert_eq!(hardcore.fees, Some(gp(0)));
    }

    #[test]
    fn fee_cap_limits_the_per_unit_fee() {
        let capped = percent_of_sale(200, Some(5));
        // 2% of 1,000 = 20, capped to 5 per unit, times 4 units.
        let outcome = evaluate(&trade(0, 1_000, 4, capped)).unwrap();
        assert_eq!(outcome.fees, Some(gp(20)));
    }

    #[test]
    fn a_cap_above_the_computed_fee_does_not_bind() {
        let capped = percent_of_sale(200, Some(1_000));
        // 2% of 1,000 = 20 per unit, well under the cap of 1,000.
        let outcome = evaluate(&trade(0, 1_000, 4, capped)).unwrap();
        assert_eq!(outcome.fees, Some(gp(80)));
    }

    #[test]
    fn a_zero_cap_removes_the_fee_without_making_it_unknown() {
        let zero_cap = percent_of_sale(200, Some(0));
        let outcome = evaluate(&trade(0, 1_000, 4, zero_cap)).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));
        assert_eq!(outcome.net_profit, Some(gp(4_000)));
    }

    #[test]
    fn unknown_fee_policy_leaves_fees_net_and_roi_unknown() {
        let outcome = evaluate(&trade(1_000, 1_100, 10, FeePolicy::Unknown)).unwrap();
        assert_eq!(outcome.gross_profit, gp(1_000));
        assert_eq!(outcome.fees, None);
        assert_eq!(outcome.net_profit, None);
        assert_eq!(outcome.roi, None);
    }

    #[test]
    fn no_fees_is_known_zero_not_unknown() {
        let outcome = evaluate(&trade(1_000, 1_100, 10, FeePolicy::NoFees)).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));
        assert_eq!(outcome.net_profit, Some(gp(1_000)));
    }

    #[test]
    fn zero_cost_leaves_roi_undefined() {
        let outcome = evaluate(&trade(0, 100, 5, FeePolicy::NoFees)).unwrap();
        assert_eq!(outcome.acquisition_cost, gp(0));
        assert_eq!(outcome.net_profit, Some(gp(500)));
        assert_eq!(outcome.roi, None);
    }

    #[test]
    fn missing_price_is_an_error_not_zero() {
        let missing_sell = Transaction::trade(Some(Price(10)), None, qty(1), FeePolicy::NoFees);
        assert_eq!(evaluate(&missing_sell), Err(ProfitError::MissingPrice));
        let missing_input = Transaction::recipe(
            vec![Line::unpriced(qty(1))],
            vec![Line::priced(Price(1), qty(1))],
            FeePolicy::NoFees,
        );
        assert_eq!(evaluate(&missing_input), Err(ProfitError::MissingPrice));
    }

    #[test]
    fn a_price_above_i64_max_is_reported_as_overflow_not_wrapped() {
        let too_large = u64::try_from(i64::MAX).unwrap() + 1;
        assert_eq!(
            evaluate(&trade(too_large, 1, 1, FeePolicy::NoFees)),
            Err(ProfitError::Overflow)
        );
        assert_eq!(
            evaluate(&trade(1, too_large, 1, standard_world_tax())),
            Err(ProfitError::Overflow)
        );
        assert_eq!(
            evaluate(&trade(1, u64::MAX, 1, FeePolicy::Unknown)),
            Err(ProfitError::Overflow)
        );
    }

    #[test]
    fn empty_input_or_output_sets_are_rejected() {
        let line = Line::priced(Price(1), qty(1));
        let no_inputs = Transaction::recipe(vec![], vec![line], FeePolicy::NoFees);
        assert_eq!(evaluate(&no_inputs), Err(ProfitError::NoInputs));
        let no_outputs = Transaction::recipe(vec![line], vec![], FeePolicy::NoFees);
        assert_eq!(evaluate(&no_outputs), Err(ProfitError::NoOutputs));
    }

    #[test]
    fn fee_rate_accepts_zero_through_one_hundred_percent() {
        assert_eq!(
            FeeRate::from_basis_points(0).map(FeeRate::basis_points),
            Ok(0)
        );
        assert_eq!(
            FeeRate::from_basis_points(200).map(FeeRate::basis_points),
            Ok(200)
        );
        assert_eq!(
            FeeRate::from_basis_points(10_000).map(FeeRate::basis_points),
            Ok(10_000)
        );
    }

    #[test]
    fn fee_rate_rejects_more_than_one_hundred_percent() {
        for basis_points in [10_001, u32::MAX] {
            assert_eq!(
                FeeRate::from_basis_points(basis_points),
                Err(FeeRateError::AboveFullRate)
            );
        }
    }

    #[test]
    fn a_zero_rate_charges_nothing_but_is_still_a_known_fee() {
        let outcome = evaluate(&trade(0, 1_000, 4, percent_of_sale(0, None))).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));
        assert_eq!(outcome.net_profit, Some(gp(4_000)));
    }

    #[test]
    fn the_overflow_free_fee_arithmetic_matches_exact_wide_arithmetic() {
        let max = i64::MAX;
        let prices = [0, 1, 49, 50, 9_999, 10_000, 10_001, max / 2, max - 1, max];
        for price in prices {
            for basis_points in [0, 1, 199, 200, 201, 9_999, 10_000] {
                let exact = i128::from(price) * i128::from(basis_points) / 10_000;
                let fee = fee_per_unit(gp(price), rate(basis_points), None);
                assert_eq!(i128::from(fee.get()), exact, "{price} at {basis_points} bp");
            }
        }
    }

    #[test]
    fn a_cap_too_large_for_gp_never_binds() {
        let uncapped = evaluate(&trade(0, 1_000, 4, two_percent())).unwrap();
        let huge_cap = evaluate(&trade(0, 1_000, 4, percent_of_sale(200, Some(u64::MAX)))).unwrap();
        assert_eq!(huge_cap.fees, uncapped.fees);
    }

    #[test]
    fn a_full_rate_fee_takes_the_whole_sale_price() {
        let all = percent_of_sale(10_000, None);
        let max = u64::try_from(i64::MAX).unwrap();
        let outcome = evaluate(&trade(0, max, 1, all)).unwrap();
        assert_eq!(outcome.fees, Some(gp(i64::MAX)));
        assert_eq!(outcome.net_profit, Some(gp(0)));
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        assert_eq!(
            evaluate(&trade(i64::MAX.unsigned_abs(), 1, 2, FeePolicy::NoFees)),
            Err(ProfitError::Overflow)
        );
        let huge_quantity = Quantity::new(u64::MAX).unwrap();
        let outcome = Transaction::trade(
            Some(Price(1)),
            Some(Price(1)),
            huge_quantity,
            FeePolicy::NoFees,
        );
        assert_eq!(evaluate(&outcome), Err(ProfitError::Overflow));
    }

    #[test]
    fn a_huge_quantity_is_fine_when_the_product_fits() {
        // Boundary correction from adopting `domain::Gp`: a quantity above
        // i64::MAX used to be reported as overflow even at a price of zero.
        let huge_quantity = Quantity::new(u64::MAX).unwrap();
        let free = Transaction::trade(
            Some(Price(0)),
            Some(Price(0)),
            huge_quantity,
            FeePolicy::NoFees,
        );
        let outcome = evaluate(&free).unwrap();
        assert_eq!(outcome.acquisition_cost, gp(0));
        assert_eq!(outcome.net_profit, Some(gp(0)));
    }

    #[test]
    fn summing_lines_can_overflow() {
        let recipe = Transaction::recipe(
            vec![
                Line::priced(Price(i64::MAX.unsigned_abs()), qty(1)),
                Line::priced(Price(1), qty(1)),
            ],
            vec![Line::priced(Price(1), qty(1))],
            FeePolicy::NoFees,
        );
        assert_eq!(evaluate(&recipe), Err(ProfitError::Overflow));
    }

    #[test]
    fn roi_is_exact_for_extreme_values() {
        let outcome = evaluate(&trade(1, i64::MAX.unsigned_abs(), 1, FeePolicy::NoFees)).unwrap();
        let expected = (i128::from(i64::MAX) - 1) * 10_000;
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(expected));
    }

    #[test]
    fn errors_have_readable_messages() {
        let messages = [
            (ProfitError::MissingPrice, "unit price is missing"),
            (ProfitError::NoInputs, "transaction has no inputs"),
            (ProfitError::NoOutputs, "transaction has no outputs"),
            (
                ProfitError::Overflow,
                "amount overflows a signed 64-bit GP value",
            ),
        ];
        for (error, message) in messages {
            assert_eq!(error.to_string(), message);
        }
        assert_eq!(
            FeeRateError::AboveFullRate.to_string(),
            "fee rate must be at most 10000 basis points"
        );
    }
}
