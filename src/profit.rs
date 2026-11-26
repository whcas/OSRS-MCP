//! Deterministic profit primitives for trades and processing recipes.
//!
//! Everything here is a pure function of explicit inputs: no network, storage,
//! clock or MCP types. Callers supply prices, quantities and a [`FeePolicy`];
//! this module only does checked integer arithmetic on them.
//!
//! # Units and arithmetic
//!
//! - Money is [`Gp`], a signed whole number of coins (`i64`). Signed so that
//!   losses are representable. Unit prices must not be negative.
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
//! # Missing data versus zero
//!
//! - A missing unit price is [`ProfitError::MissingPrice`], never treated as
//!   zero. A free input is expressed by an explicit price of `Gp::new(0)`.
//! - An unknown fee policy ([`FeePolicy::Unknown`]) leaves fees, net profit and
//!   ROI as `None` in the [`Outcome`], while gross figures remain available.
//!   [`FeePolicy::NoFees`] is a deliberate assertion that no fee applies.
//! - ROI is `None` when the acquisition cost is zero, since it is undefined
//!   rather than zero.

use std::fmt;

/// Divisor converting basis points to a fraction.
const BASIS_POINTS_PER_UNIT: i128 = 10_000;

/// A signed amount of coins (GP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

    fn checked_add(self, other: Self) -> Result<Self, ProfitError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(ProfitError::Overflow)
    }

    fn checked_sub(self, other: Self) -> Result<Self, ProfitError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or(ProfitError::Overflow)
    }

    fn checked_mul_quantity(self, quantity: Quantity) -> Result<Self, ProfitError> {
        let quantity = i64::try_from(quantity.0).map_err(|_| ProfitError::Overflow)?;
        self.0
            .checked_mul(quantity)
            .map(Self)
            .ok_or(ProfitError::Overflow)
    }
}

/// A strictly positive item count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Quantity(u64);

impl Quantity {
    /// Creates a quantity, rejecting zero.
    ///
    /// # Errors
    ///
    /// Returns [`ProfitError::ZeroQuantity`] if `count` is zero.
    pub const fn new(count: u64) -> Result<Self, ProfitError> {
        if count == 0 {
            Err(ProfitError::ZeroQuantity)
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

/// One line of a transaction: a quantity of items at a unit price.
///
/// The price is optional so that a missing observation stays distinguishable
/// from a price of zero; evaluation fails on a missing price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    unit_price: Option<Gp>,
    quantity: Quantity,
}

impl Line {
    /// A line with a known unit price.
    #[must_use]
    pub const fn priced(unit_price: Gp, quantity: Quantity) -> Self {
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
        /// Rate in basis points, from 0 to 10,000 inclusive.
        rate_basis_points: u32,
        /// Optional ceiling on the fee for a single unit, applied after
        /// rounding down.
        cap_per_unit: Option<Gp>,
    },
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
        buy_price: Option<Gp>,
        sell_price: Option<Gp>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfitError {
    /// A quantity of zero was supplied.
    ZeroQuantity,
    /// A unit price was negative.
    NegativePrice,
    /// A unit price was missing (as opposed to zero).
    MissingPrice,
    /// The transaction has no input lines.
    NoInputs,
    /// The transaction has no output lines.
    NoOutputs,
    /// A fee rate above 10,000 basis points or a negative fee cap.
    InvalidFeePolicy,
    /// An intermediate value did not fit in a signed 64-bit amount.
    Overflow,
}

impl fmt::Display for ProfitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroQuantity => "quantity must be greater than zero",
            Self::NegativePrice => "unit price must not be negative",
            Self::MissingPrice => "unit price is missing",
            Self::NoInputs => "transaction has no inputs",
            Self::NoOutputs => "transaction has no outputs",
            Self::InvalidFeePolicy => {
                "fee policy is invalid: rate must be at most 10000 bp and cap non-negative"
            }
            Self::Overflow => "amount overflows a signed 64-bit GP value",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ProfitError {}

/// Evaluates a transaction.
///
/// # Errors
///
/// Returns a [`ProfitError`] for missing or negative prices, empty input or
/// output sets, an invalid fee policy, or arithmetic overflow.
pub fn evaluate(transaction: &Transaction) -> Result<Outcome, ProfitError> {
    if transaction.inputs.is_empty() {
        return Err(ProfitError::NoInputs);
    }
    if transaction.outputs.is_empty() {
        return Err(ProfitError::NoOutputs);
    }
    validate_fee_policy(transaction.fees)?;

    let acquisition_cost = total_value(&transaction.inputs)?;
    let gross_revenue = total_value(&transaction.outputs)?;
    let gross_profit = gross_revenue.checked_sub(acquisition_cost)?;

    let fees = total_fees(&transaction.outputs, transaction.fees)?;
    let net_profit = fees
        .map(|fees| gross_profit.checked_sub(fees))
        .transpose()?;
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

fn validate_fee_policy(policy: FeePolicy) -> Result<(), ProfitError> {
    match policy {
        FeePolicy::PercentOfSale {
            rate_basis_points,
            cap_per_unit,
        } => {
            let rate_ok = i128::from(rate_basis_points) <= BASIS_POINTS_PER_UNIT;
            let cap_ok = cap_per_unit.is_none_or(|cap| cap.get() >= 0);
            if rate_ok && cap_ok {
                Ok(())
            } else {
                Err(ProfitError::InvalidFeePolicy)
            }
        }
        FeePolicy::Unknown | FeePolicy::NoFees => Ok(()),
    }
}

/// A line's unit price, rejecting missing and negative values.
const fn unit_price(line: &Line) -> Result<Gp, ProfitError> {
    match line.unit_price {
        None => Err(ProfitError::MissingPrice),
        Some(price) if price.get() < 0 => Err(ProfitError::NegativePrice),
        Some(price) => Ok(price),
    }
}

fn total_value(lines: &[Line]) -> Result<Gp, ProfitError> {
    lines.iter().try_fold(Gp::new(0), |total, line| {
        total.checked_add(unit_price(line)?.checked_mul_quantity(line.quantity)?)
    })
}

/// Total fees on the sold lines; `None` when the policy is unknown.
fn total_fees(sold: &[Line], policy: FeePolicy) -> Result<Option<Gp>, ProfitError> {
    match policy {
        FeePolicy::Unknown => Ok(None),
        FeePolicy::NoFees => Ok(Some(Gp::new(0))),
        FeePolicy::PercentOfSale {
            rate_basis_points,
            cap_per_unit,
        } => sold
            .iter()
            .try_fold(Gp::new(0), |total, line| {
                let per_unit = fee_per_unit(unit_price(line)?, rate_basis_points, cap_per_unit)?;
                total.checked_add(per_unit.checked_mul_quantity(line.quantity)?)
            })
            .map(Some),
    }
}

/// Floor of `price * rate`, then capped. The product is held in `i128`, so it
/// cannot overflow; the rate is at most 100%, so the result fits back in `i64`.
fn fee_per_unit(price: Gp, rate_basis_points: u32, cap: Option<Gp>) -> Result<Gp, ProfitError> {
    let fee = i128::from(price.get()) * i128::from(rate_basis_points) / BASIS_POINTS_PER_UNIT;
    let fee = i64::try_from(fee).map_err(|_| ProfitError::Overflow)?;
    Ok(Gp::new(cap.map_or(fee, |cap| fee.min(cap.get()))))
}

fn return_on_investment(net_profit: Gp, cost: Gp) -> Option<BasisPoints> {
    if cost.get() == 0 {
        return None;
    }
    let scaled = i128::from(net_profit.get()) * BASIS_POINTS_PER_UNIT;
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

    /// An illustrative 2% policy used to exercise the arithmetic. It is a test
    /// input only, not a claim about any world's fee rules.
    const TWO_PERCENT: FeePolicy = FeePolicy::PercentOfSale {
        rate_basis_points: 200,
        cap_per_unit: None,
    };

    fn trade(buy: i64, sell: i64, count: u64, fees: FeePolicy) -> Transaction {
        Transaction::trade(Some(gp(buy)), Some(gp(sell)), qty(count), fees)
    }

    #[test]
    fn worked_trade_with_percentage_fee() {
        // Buy 100 at 1,000 = 100,000. Sell 100 at 1,100 = 110,000.
        // Fee per unit = floor(1,100 * 2%) = 22, so 2,200 in total.
        // Net = 110,000 - 100,000 - 2,200 = 7,800, which is 7.8% = 780 bp.
        let outcome = evaluate(&trade(1_000, 1_100, 100, TWO_PERCENT)).unwrap();
        assert_eq!(outcome.acquisition_cost, gp(100_000));
        assert_eq!(outcome.gross_revenue, gp(110_000));
        assert_eq!(outcome.gross_profit, gp(10_000));
        assert_eq!(outcome.fees, Some(gp(2_200)));
        assert_eq!(outcome.net_profit, Some(gp(7_800)));
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(780));
    }

    #[test]
    fn worked_multi_input_recipe() {
        // Inputs: 2 x A at 300, 1 x B at 250, 1 x C owned (explicit price 0).
        // Cost = 600 + 250 + 0 = 850.
        // Outputs: 3 x D at 400 (1,200) and 1 x E at 99 (99) = 1,299.
        // Fees at 2% floored per unit: D 8 each = 24, E floor(1.98) = 1 -> 25.
        // Net = 1,299 - 850 - 25 = 424. ROI = 424 * 10,000 / 850 = 4,988 bp.
        let recipe = Transaction::recipe(
            vec![
                Line::priced(gp(300), qty(2)),
                Line::priced(gp(250), qty(1)),
                Line::priced(gp(0), qty(1)),
            ],
            vec![Line::priced(gp(400), qty(3)), Line::priced(gp(99), qty(1))],
            TWO_PERCENT,
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
        let outcome = evaluate(&trade(10, 49, 1_000, TWO_PERCENT)).unwrap();
        assert_eq!(outcome.fees, Some(gp(0)));
    }

    #[test]
    fn fee_cap_limits_the_per_unit_fee() {
        let capped = FeePolicy::PercentOfSale {
            rate_basis_points: 200,
            cap_per_unit: Some(gp(5)),
        };
        // 2% of 1,000 = 20, capped to 5 per unit, times 4 units.
        let outcome = evaluate(&trade(0, 1_000, 4, capped)).unwrap();
        assert_eq!(outcome.fees, Some(gp(20)));
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
    fn zero_quantity_is_rejected() {
        assert_eq!(Quantity::new(0), Err(ProfitError::ZeroQuantity));
    }

    #[test]
    fn missing_price_is_an_error_not_zero() {
        let missing_sell = Transaction::trade(Some(gp(10)), None, qty(1), FeePolicy::NoFees);
        assert_eq!(evaluate(&missing_sell), Err(ProfitError::MissingPrice));
        let missing_input = Transaction::recipe(
            vec![Line::unpriced(qty(1))],
            vec![Line::priced(gp(1), qty(1))],
            FeePolicy::NoFees,
        );
        assert_eq!(evaluate(&missing_input), Err(ProfitError::MissingPrice));
    }

    #[test]
    fn negative_price_is_rejected() {
        assert_eq!(
            evaluate(&trade(-1, 10, 1, FeePolicy::NoFees)),
            Err(ProfitError::NegativePrice)
        );
        assert_eq!(
            evaluate(&trade(1, -10, 1, FeePolicy::NoFees)),
            Err(ProfitError::NegativePrice)
        );
    }

    #[test]
    fn empty_input_or_output_sets_are_rejected() {
        let line = Line::priced(gp(1), qty(1));
        let no_inputs = Transaction::recipe(vec![], vec![line], FeePolicy::NoFees);
        assert_eq!(evaluate(&no_inputs), Err(ProfitError::NoInputs));
        let no_outputs = Transaction::recipe(vec![line], vec![], FeePolicy::NoFees);
        assert_eq!(evaluate(&no_outputs), Err(ProfitError::NoOutputs));
    }

    #[test]
    fn invalid_fee_policies_are_rejected() {
        let too_high = FeePolicy::PercentOfSale {
            rate_basis_points: 10_001,
            cap_per_unit: None,
        };
        assert_eq!(
            evaluate(&trade(1, 2, 1, too_high)),
            Err(ProfitError::InvalidFeePolicy)
        );
        let negative_cap = FeePolicy::PercentOfSale {
            rate_basis_points: 100,
            cap_per_unit: Some(gp(-1)),
        };
        assert_eq!(
            evaluate(&trade(1, 2, 1, negative_cap)),
            Err(ProfitError::InvalidFeePolicy)
        );
    }

    #[test]
    fn a_full_rate_fee_takes_the_whole_sale_price() {
        let all = FeePolicy::PercentOfSale {
            rate_basis_points: 10_000,
            cap_per_unit: None,
        };
        let outcome = evaluate(&trade(0, i64::MAX, 1, all)).unwrap();
        assert_eq!(outcome.fees, Some(gp(i64::MAX)));
        assert_eq!(outcome.net_profit, Some(gp(0)));
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        assert_eq!(
            evaluate(&trade(i64::MAX, 1, 2, FeePolicy::NoFees)),
            Err(ProfitError::Overflow)
        );
        let huge_quantity = Quantity::new(u64::MAX).unwrap();
        let outcome =
            Transaction::trade(Some(gp(1)), Some(gp(1)), huge_quantity, FeePolicy::NoFees);
        assert_eq!(evaluate(&outcome), Err(ProfitError::Overflow));
    }

    #[test]
    fn summing_lines_can_overflow() {
        let recipe = Transaction::recipe(
            vec![
                Line::priced(gp(i64::MAX), qty(1)),
                Line::priced(gp(1), qty(1)),
            ],
            vec![Line::priced(gp(1), qty(1))],
            FeePolicy::NoFees,
        );
        assert_eq!(evaluate(&recipe), Err(ProfitError::Overflow));
    }

    #[test]
    fn roi_is_exact_for_extreme_values() {
        let outcome = evaluate(&trade(1, i64::MAX, 1, FeePolicy::NoFees)).unwrap();
        let expected = (i128::from(i64::MAX) - 1) * 10_000;
        assert_eq!(outcome.roi.map(BasisPoints::get), Some(expected));
    }

    #[test]
    fn errors_have_readable_messages() {
        assert_eq!(
            ProfitError::MissingPrice.to_string(),
            "unit price is missing"
        );
    }
}
