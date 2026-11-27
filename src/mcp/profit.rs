//! The `calculate_profit` tool: deterministic profit for a proposed trade or
//! processing recipe.
//!
//! A thin adapter over [`crate::profit`]. It validates and converts the request,
//! calls [`evaluate`], and converts the outcome, with no arithmetic of its own.
//! It uses no network and no clock.
//!
//! # Integer range
//!
//! JSON numbers above 2^53 − 1 lose precision in many clients. The domain types
//! are wider, so this adapter rejects any request integer, and any result, that
//! is outside ±([`MAX_EXACT_INTEGER`]). A domain-valid result that is not exactly
//! representable is an error rather than a rounded number.

use rmcp::{Json, handler::server::wrapper::Parameters, schemars::JsonSchema, tool, tool_router};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::OsrsMcpServer;
use crate::domain::{DomainError, Gp, Price, Quantity};
use crate::profit::{
    FeePolicy, FeeRate, FeeRateError, Line, Outcome, ProfitError, Transaction, evaluate,
};

/// The largest integer a JSON number carries exactly in common clients (2^53 − 1).
const MAX_EXACT_INTEGER: u64 = (1 << 53) - 1;

/// Shown with every result so arithmetic on supplied prices is not read as a
/// forecast.
const PROFIT_NOTE: &str = "Arithmetic on the prices you supplied. It does not predict fills, \
realised profit, fill rate or GP/hour. fees_gp, net_profit_gp and roi_basis_points are null when \
the fee policy is unknown; roi_basis_points is also null when the acquisition cost is zero.";

/// Most lines accepted on each side of a recipe, which keeps requests small.
const MAX_RECIPE_LINES: usize = 50;

/// Why a request could not be turned into a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
enum CalculationError {
    /// Neither or both of `trade` and `recipe` were given.
    #[error("provide exactly one of `trade` and `recipe`")]
    TradeOrRecipe,
    /// A recipe side has more lines than [`MAX_RECIPE_LINES`].
    #[error("`{side}` must have at most {MAX_RECIPE_LINES} lines")]
    TooManyLines { side: &'static str },
    /// A request integer is above the exactly representable range.
    #[error("`{field}` must be at most {MAX_EXACT_INTEGER} so it is represented exactly")]
    InputOutOfRange { field: &'static str },
    /// The quantity was not a valid count.
    #[error("{0}")]
    Quantity(DomainError),
    /// The fee rate was above 100%.
    #[error("`rate_basis_points` must be at most 10000 (100%)")]
    FeeRateAboveFull,
    /// The evaluator rejected the transaction.
    #[error("{0}")]
    Profit(#[from] ProfitError),
    /// A result is domain-valid but not exactly representable as a JSON integer.
    #[error(
        "`{field}` is outside ±{MAX_EXACT_INTEGER}, so it cannot be returned exactly; \
         use smaller prices or quantities"
    )]
    ResultOutOfRange { field: &'static str },
}

impl From<FeeRateError> for CalculationError {
    fn from(error: FeeRateError) -> Self {
        match error {
            FeeRateError::AboveFullRate => Self::FeeRateAboveFull,
        }
    }
}

/// The fee rules to apply to the sale. There is no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FeesInput {
    /// The fee rules are not known. Fees, net profit and ROI are returned as null.
    Unknown,
    /// Asserts that selling incurs no fee. Hardcore Main Worlds were observed to
    /// charge none (maintainer, in game, 2026-11-26); nothing assumes it for you.
    None,
    /// A percentage of each unit's sale price, rounded down per unit, optionally
    /// capped per unit.
    PercentOfSale {
        /// The rate in basis points, 0 to 10000 (100 bp = 1%, so 200 is 2%).
        rate_basis_points: u32,
        /// Optional ceiling on the fee for one unit, in whole GP, applied after
        /// rounding down.
        cap_per_unit_gp: Option<u64>,
    },
}

/// The trade to evaluate: buy and later sell the same quantity.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct TradeInput {
    /// Price paid per item, in whole GP. Zero is allowed and means a price of zero.
    buy_price_gp: u64,
    /// Price received per item before fees, in whole GP. Zero is allowed.
    sell_price_gp: u64,
    /// Number of items bought and sold, at least 1.
    quantity: u64,
}

/// One input or output line of a recipe.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct LineInput {
    /// Price per item, in whole GP. Zero is allowed and means a price of zero. An
    /// item you already own still has an opportunity cost, so price it deliberately.
    unit_price_gp: u64,
    /// Number of items in this line, at least 1.
    quantity: u64,
}

/// A processing recipe: items consumed and items produced and sold.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct RecipeInput {
    /// Items consumed, 1 to 50 lines. Priced as acquired; no fee applies.
    inputs: Vec<LineInput>,
    /// Items produced and sold, 1 to 50 lines. The fee policy applies to each.
    outputs: Vec<LineInput>,
}

/// Arguments of the `calculate_profit` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
struct CalculateProfitInput {
    /// A trade to evaluate, using prices you choose. Give exactly one of `trade`
    /// and `recipe`.
    trade: Option<TradeInput>,
    /// A processing recipe to evaluate, using prices you choose. Give exactly one
    /// of `trade` and `recipe`.
    recipe: Option<RecipeInput>,
    /// How fees on the sale are determined. Required: no fee rule is assumed.
    fees: FeesInput,
}

/// Result of the `calculate_profit` tool. All amounts are whole GP.
#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct CalculateProfitOutput {
    /// Total paid: buy price times quantity.
    acquisition_cost_gp: i64,
    /// Total received before fees: sell price times quantity.
    gross_revenue_gp: i64,
    /// Gross revenue minus acquisition cost. Negative for a loss.
    gross_profit_gp: i64,
    /// Total fees, or null if the fee policy is unknown.
    fees_gp: Option<i64>,
    /// Gross profit minus fees, or null if the fee policy is unknown. Negative
    /// for a loss.
    net_profit_gp: Option<i64>,
    /// Net profit divided by acquisition cost, in basis points (100 bp = 1%),
    /// truncated toward zero. Null if fees are unknown or the cost is zero.
    roi_basis_points: Option<i64>,
    /// The fee policy that was applied, echoed back.
    fees: FeesInput,
    /// What this result does and does not mean.
    note: &'static str,
}

/// A request integer, rejected above the exactly representable range.
const fn bounded(value: u64, field: &'static str) -> Result<u64, CalculationError> {
    if value > MAX_EXACT_INTEGER {
        Err(CalculationError::InputOutOfRange { field })
    } else {
        Ok(value)
    }
}

fn fee_policy(fees: FeesInput) -> Result<FeePolicy, CalculationError> {
    Ok(match fees {
        FeesInput::Unknown => FeePolicy::Unknown,
        FeesInput::None => FeePolicy::NoFees,
        FeesInput::PercentOfSale {
            rate_basis_points,
            cap_per_unit_gp,
        } => FeePolicy::PercentOfSale {
            rate: FeeRate::from_basis_points(rate_basis_points)?,
            cap_per_unit: cap_per_unit_gp
                .map(|cap| bounded(cap, "cap_per_unit_gp").map(Price))
                .transpose()?,
        },
    })
}

fn transaction(input: &CalculateProfitInput) -> Result<Transaction, CalculationError> {
    match (&input.trade, &input.recipe) {
        (Some(trade), None) => trade_transaction(trade, input.fees),
        (None, Some(recipe)) => recipe_transaction(recipe, input.fees),
        _ => Err(CalculationError::TradeOrRecipe),
    }
}

fn trade_transaction(trade: &TradeInput, fees: FeesInput) -> Result<Transaction, CalculationError> {
    let buy = Price(bounded(trade.buy_price_gp, "buy_price_gp")?);
    let sell = Price(bounded(trade.sell_price_gp, "sell_price_gp")?);
    let quantity =
        Quantity::new(bounded(trade.quantity, "quantity")?).map_err(CalculationError::Quantity)?;
    Ok(Transaction::trade(
        Some(buy),
        Some(sell),
        quantity,
        fee_policy(fees)?,
    ))
}

fn recipe_transaction(
    recipe: &RecipeInput,
    fees: FeesInput,
) -> Result<Transaction, CalculationError> {
    let inputs = lines(&recipe.inputs, "inputs")?;
    let outputs = lines(&recipe.outputs, "outputs")?;
    Ok(Transaction::recipe(inputs, outputs, fee_policy(fees)?))
}

/// Converts one side of a recipe. An empty side is left for the evaluator to
/// reject, so it stays a distinct error.
fn lines(side: &[LineInput], name: &'static str) -> Result<Vec<Line>, CalculationError> {
    if side.len() > MAX_RECIPE_LINES {
        return Err(CalculationError::TooManyLines { side: name });
    }
    side.iter()
        .map(|line| {
            let price = bounded(line.unit_price_gp, "unit_price_gp")?;
            let quantity = bounded(line.quantity, "quantity")?;
            let quantity = Quantity::new(quantity).map_err(CalculationError::Quantity)?;
            Ok(Line::priced(Price(price), quantity))
        })
        .collect()
}

/// A result amount, rejected outside the exactly representable range.
const fn exact_amount(amount: Gp, field: &'static str) -> Result<i64, CalculationError> {
    let value = amount.get();
    if value.unsigned_abs() > MAX_EXACT_INTEGER {
        Err(CalculationError::ResultOutOfRange { field })
    } else {
        Ok(value)
    }
}

fn exact_roi(roi: i128) -> Result<i64, CalculationError> {
    i64::try_from(roi)
        .ok()
        .filter(|value| value.unsigned_abs() <= MAX_EXACT_INTEGER)
        .ok_or(CalculationError::ResultOutOfRange {
            field: "roi_basis_points",
        })
}

fn output(outcome: Outcome, fees: FeesInput) -> Result<CalculateProfitOutput, CalculationError> {
    let optional =
        |amount: Option<Gp>, field| amount.map(|amount| exact_amount(amount, field)).transpose();
    Ok(CalculateProfitOutput {
        acquisition_cost_gp: exact_amount(outcome.acquisition_cost, "acquisition_cost_gp")?,
        gross_revenue_gp: exact_amount(outcome.gross_revenue, "gross_revenue_gp")?,
        gross_profit_gp: exact_amount(outcome.gross_profit, "gross_profit_gp")?,
        fees_gp: optional(outcome.fees, "fees_gp")?,
        net_profit_gp: optional(outcome.net_profit, "net_profit_gp")?,
        roi_basis_points: outcome.roi.map(|roi| exact_roi(roi.get())).transpose()?,
        fees,
        note: PROFIT_NOTE,
    })
}

fn calculate(input: &CalculateProfitInput) -> Result<CalculateProfitOutput, CalculationError> {
    let outcome = evaluate(&transaction(input)?)?;
    output(outcome, input.fees)
}

#[tool_router(router = profit_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Evaluates a proposed trade with explicit prices and an explicit fee policy.
    #[tool(
        name = "calculate_profit",
        description = "Deterministic profit for a proposed trade (buy and sell a quantity) or \
                       processing recipe (input and output lines, each with a unit price and \
                       quantity; give exactly one of trade and recipe) at prices you supply (whole GP) under an explicit fee policy (unknown, none, \
                       or a percentage of each unit's sale price with an optional per-unit cap). \
                       Returns acquisition cost, gross revenue and profit, fees, net profit and \
                       ROI in basis points, with losses negative. No fee rule is assumed; \
                       Hardcore Main Worlds were observed to charge none, so pass kind=none \
                       only when you mean it. Uses no network and no market data. Observed \
                       instant-buy and instant-sell prices are not guaranteed fills; this is \
                       arithmetic, not a forecast.",
        annotations(
            title = "Calculate profit",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn calculate_profit(
        Parameters(input): Parameters<CalculateProfitInput>,
    ) -> Result<Json<CalculateProfitOutput>, String> {
        calculate(&input)
            .map(Json)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use rmcp::{
        ServiceExt,
        model::{CallToolRequestParams, CallToolResult},
        service::{RoleClient, RunningService},
    };
    use serde_json::{Value, json};

    use super::*;

    fn input(buy: u64, sell: u64, quantity: u64, fees: FeesInput) -> CalculateProfitInput {
        CalculateProfitInput {
            trade: Some(TradeInput {
                buy_price_gp: buy,
                sell_price_gp: sell,
                quantity,
            }),
            recipe: None,
            fees,
        }
    }

    fn line(unit_price_gp: u64, quantity: u64) -> LineInput {
        LineInput {
            unit_price_gp,
            quantity,
        }
    }

    fn recipe(
        inputs: Vec<LineInput>,
        outputs: Vec<LineInput>,
        fees: FeesInput,
    ) -> CalculateProfitInput {
        CalculateProfitInput {
            trade: None,
            recipe: Some(RecipeInput { inputs, outputs }),
            fees,
        }
    }

    fn percent(rate_basis_points: u32, cap_per_unit_gp: Option<u64>) -> FeesInput {
        FeesInput::PercentOfSale {
            rate_basis_points,
            cap_per_unit_gp,
        }
    }

    fn calc(input: &CalculateProfitInput) -> CalculateProfitOutput {
        calculate(input).unwrap()
    }

    #[test]
    fn a_profitable_trade_with_no_fees_reports_every_figure_in_gp_and_basis_points() {
        let result = calc(&input(100, 150, 10, FeesInput::None));

        assert_eq!(result.acquisition_cost_gp, 1_000);
        assert_eq!(result.gross_revenue_gp, 1_500);
        assert_eq!(result.gross_profit_gp, 500);
        assert_eq!(result.fees_gp, Some(0));
        assert_eq!(result.net_profit_gp, Some(500));
        assert_eq!(result.roi_basis_points, Some(5_000));
        assert_eq!(result.fees, FeesInput::None);
    }

    #[test]
    fn a_losing_trade_is_negative_and_roi_truncates_toward_zero() {
        let result = calc(&input(3, 2, 1, FeesInput::None));

        assert_eq!(result.net_profit_gp, Some(-1));
        assert_eq!(result.roi_basis_points, Some(-3_333));
    }

    #[test]
    fn unknown_fees_leave_fees_net_and_roi_unknown_but_keep_gross_figures() {
        let result = calc(&input(100, 150, 10, FeesInput::Unknown));

        assert_eq!(result.gross_profit_gp, 500);
        assert_eq!(result.fees_gp, None);
        assert_eq!(result.net_profit_gp, None);
        assert_eq!(result.roi_basis_points, None);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["fees_gp"], Value::Null, "unknown is explicit null");
    }

    #[test]
    fn percentage_fees_round_down_per_unit_and_respect_the_cap() {
        // 2% of 99 is 1.98, floored to 1 per unit; 10 units.
        let rounded = calc(&input(0, 99, 10, percent(200, None)));
        assert_eq!(rounded.fees_gp, Some(10));

        // 2% of 1,000 is 20 per unit, capped at 5.
        let capped = calc(&input(0, 1_000, 10, percent(200, Some(5))));
        assert_eq!(capped.fees_gp, Some(50));
        assert_eq!(capped.net_profit_gp, Some(9_950));
    }

    #[test]
    fn zero_acquisition_cost_leaves_roi_undefined_not_zero() {
        let result = calc(&input(0, 10, 1, FeesInput::None));

        assert_eq!(result.net_profit_gp, Some(10));
        assert_eq!(result.roi_basis_points, None);
    }

    #[test]
    fn a_zero_price_is_a_real_price_of_zero() {
        let result = calc(&input(5, 0, 2, FeesInput::None));

        assert_eq!(result.gross_revenue_gp, 0);
        assert_eq!(result.gross_profit_gp, -10);
    }

    #[test]
    fn invalid_requests_are_rejected_with_specific_errors() {
        let error = |input: &CalculateProfitInput| calculate(input).unwrap_err();

        assert_eq!(
            error(&input(1, 2, 0, FeesInput::None)),
            CalculationError::Quantity(DomainError::ZeroQuantity)
        );
        assert_eq!(
            error(&input(1, 2, 1, percent(10_001, None))),
            CalculationError::FeeRateAboveFull
        );
        assert_eq!(
            error(&input(MAX_EXACT_INTEGER + 1, 2, 1, FeesInput::None)),
            CalculationError::InputOutOfRange {
                field: "buy_price_gp"
            }
        );
        assert_eq!(
            error(&input(1, MAX_EXACT_INTEGER + 1, 1, FeesInput::None)),
            CalculationError::InputOutOfRange {
                field: "sell_price_gp"
            }
        );
        assert_eq!(
            error(&input(1, 2, MAX_EXACT_INTEGER + 1, FeesInput::None)),
            CalculationError::InputOutOfRange { field: "quantity" }
        );
        assert_eq!(
            error(&input(1, 2, 1, percent(200, Some(MAX_EXACT_INTEGER + 1)))),
            CalculationError::InputOutOfRange {
                field: "cap_per_unit_gp"
            }
        );
    }

    #[test]
    fn the_largest_exactly_representable_inputs_are_accepted() {
        let result = calc(&input(0, 1, MAX_EXACT_INTEGER, FeesInput::None));

        assert_eq!(
            result.gross_revenue_gp,
            i64::try_from(MAX_EXACT_INTEGER).unwrap()
        );
    }

    #[test]
    fn domain_overflow_is_reported_distinctly_from_the_range_guard() {
        // Each input is within range, but price times quantity overflows i64.
        let result = calculate(&input(
            MAX_EXACT_INTEGER,
            MAX_EXACT_INTEGER,
            MAX_EXACT_INTEGER,
            FeesInput::None,
        ));

        assert_eq!(result, Err(CalculationError::Profit(ProfitError::Overflow)));
    }

    #[test]
    fn a_domain_valid_result_beyond_the_exact_range_is_an_error_not_a_rounded_number() {
        // 4e15 * 4 = 1.6e16 fits i64 but exceeds 2^53 - 1.
        let result = calculate(&input(0, 4_000_000_000_000_000, 4, FeesInput::None));

        assert_eq!(
            result,
            Err(CalculationError::ResultOutOfRange {
                field: "gross_revenue_gp"
            })
        );
    }

    #[test]
    fn an_roi_beyond_the_exact_range_is_an_error_even_when_every_amount_fits() {
        // Profit just under 2^53 on a cost of 1 gives an ROI near 9e19 bp.
        let result = calculate(&input(1, MAX_EXACT_INTEGER - 1, 1, FeesInput::None));

        assert_eq!(
            result,
            Err(CalculationError::ResultOutOfRange {
                field: "roi_basis_points"
            })
        );
    }

    #[test]
    fn error_messages_are_fixed_text_naming_the_field_without_echoing_values() {
        let message = CalculationError::InputOutOfRange {
            field: "buy_price_gp",
        }
        .to_string();

        assert_eq!(
            message,
            "`buy_price_gp` must be at most 9007199254740991 so it is represented exactly"
        );
        assert_eq!(
            CalculationError::Profit(ProfitError::Overflow).to_string(),
            "amount overflows a signed 64-bit GP value"
        );
    }

    #[test]
    fn a_recipe_sums_lines_and_applies_fees_to_outputs_only() {
        // Cost 2*100 + 50 = 250; revenue 3*200 = 600; 2% fee of 200 is 4 per unit.
        let result = calc(&recipe(
            vec![line(100, 2), line(50, 1)],
            vec![line(200, 3)],
            percent(200, None),
        ));

        assert_eq!(result.acquisition_cost_gp, 250);
        assert_eq!(result.gross_revenue_gp, 600);
        assert_eq!(result.gross_profit_gp, 350);
        assert_eq!(result.fees_gp, Some(12));
        assert_eq!(result.net_profit_gp, Some(338));
        assert_eq!(result.roi_basis_points, Some(13_520));
    }

    #[test]
    fn a_recipe_matches_the_equivalent_trade() {
        let as_trade = calc(&input(100, 150, 10, percent(200, Some(2))));
        let as_recipe = calc(&recipe(
            vec![line(100, 10)],
            vec![line(150, 10)],
            percent(200, Some(2)),
        ));

        assert_eq!(as_trade, as_recipe);
    }

    #[test]
    fn exactly_one_of_trade_and_recipe_is_required() {
        let mut both = input(1, 2, 1, FeesInput::None);
        both.recipe = Some(RecipeInput {
            inputs: vec![line(1, 1)],
            outputs: vec![line(2, 1)],
        });
        let mut neither = input(1, 2, 1, FeesInput::None);
        neither.trade = None;

        assert_eq!(calculate(&both), Err(CalculationError::TradeOrRecipe));
        assert_eq!(calculate(&neither), Err(CalculationError::TradeOrRecipe));
    }

    #[test]
    fn empty_recipe_sides_are_distinct_errors_not_zero() {
        assert_eq!(
            calculate(&recipe(vec![], vec![line(1, 1)], FeesInput::None)),
            Err(CalculationError::Profit(ProfitError::NoInputs))
        );
        assert_eq!(
            calculate(&recipe(vec![line(1, 1)], vec![], FeesInput::None)),
            Err(CalculationError::Profit(ProfitError::NoOutputs))
        );
    }

    #[test]
    fn recipe_sides_are_limited_to_fifty_lines() {
        let many = |count: usize| (0..count).map(|_| line(1, 1)).collect::<Vec<_>>();

        assert!(calculate(&recipe(many(50), many(50), FeesInput::None)).is_ok());
        assert_eq!(
            calculate(&recipe(many(51), many(1), FeesInput::None)),
            Err(CalculationError::TooManyLines { side: "inputs" })
        );
        assert_eq!(
            calculate(&recipe(many(1), many(51), FeesInput::None)),
            Err(CalculationError::TooManyLines { side: "outputs" })
        );
    }

    #[test]
    fn recipe_lines_use_the_same_range_and_quantity_checks() {
        assert_eq!(
            calculate(&recipe(vec![line(1, 0)], vec![line(1, 1)], FeesInput::None)),
            Err(CalculationError::Quantity(DomainError::ZeroQuantity))
        );
        assert_eq!(
            calculate(&recipe(
                vec![line(1, 1)],
                vec![line(MAX_EXACT_INTEGER + 1, 1)],
                FeesInput::None
            )),
            Err(CalculationError::InputOutOfRange {
                field: "unit_price_gp"
            })
        );
        // Sum of results beyond the exact range is an error, not a rounded number.
        assert_eq!(
            calculate(&recipe(
                vec![line(0, 1)],
                vec![line(4_000_000_000_000_000, 4)],
                FeesInput::None
            )),
            Err(CalculationError::ResultOutOfRange {
                field: "gross_revenue_gp"
            })
        );
    }

    async fn connect() -> RunningService<RoleClient, ()> {
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let server = OsrsMcpServer::new();
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        ().serve(client_io).await.expect("client should initialise")
    }

    async fn call(client: &RunningService<RoleClient, ()>, arguments: Value) -> CallToolResult {
        let arguments = arguments.as_object().expect("object arguments").clone();
        client
            .call_tool(CallToolRequestParams::new("calculate_profit").with_arguments(arguments))
            .await
            .expect("tool call should complete")
    }

    #[tokio::test]
    async fn tool_is_discoverable_read_only_closed_world_and_describes_its_input() {
        let client = connect().await;

        let tools = client.list_all_tools().await.expect("tools/list");

        let tool = tools
            .iter()
            .find(|tool| tool.name == "calculate_profit")
            .expect("calculate_profit is registered");
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(schema["required"], json!(["fees"]));
        assert!(tool.output_schema.is_some(), "typed output schema");
    }

    #[tokio::test]
    async fn a_call_returns_structured_results_for_a_loss_and_for_unknown_fees() {
        let client = connect().await;

        let loss = call(
            &client,
            json!({
                "trade": { "buy_price_gp": 120, "sell_price_gp": 100, "quantity": 5 },
                "fees": { "kind": "none" }
            }),
        )
        .await;
        let unknown = call(
            &client,
            json!({
                "trade": { "buy_price_gp": 100, "sell_price_gp": 120, "quantity": 5 },
                "fees": { "kind": "unknown" }
            }),
        )
        .await;

        let loss = loss.structured_content.expect("structured content");
        assert_eq!(loss["net_profit_gp"], -100);
        assert_eq!(loss["roi_basis_points"], -1_666);
        let unknown = unknown.structured_content.expect("structured content");
        assert_eq!(unknown["gross_profit_gp"], 100);
        assert_eq!(unknown["net_profit_gp"], Value::Null);
        assert_eq!(unknown["fees"], json!({ "kind": "unknown" }));
    }

    #[tokio::test]
    async fn a_recipe_is_evaluated_over_the_wire_and_both_forms_together_are_an_error() {
        let client = connect().await;
        let recipe = json!({
            "inputs": [{ "unit_price_gp": 100, "quantity": 2 }],
            "outputs": [{ "unit_price_gp": 300, "quantity": 1 }]
        });

        let result = call(
            &client,
            json!({ "recipe": recipe, "fees": { "kind": "none" } }),
        )
        .await;
        let both = call(
            &client,
            json!({
                "recipe": recipe,
                "trade": { "buy_price_gp": 1, "sell_price_gp": 2, "quantity": 1 },
                "fees": { "kind": "none" }
            }),
        )
        .await;

        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["acquisition_cost_gp"], 200);
        assert_eq!(structured["net_profit_gp"], 100);
        assert_eq!(structured["roi_basis_points"], 5_000);
        assert_eq!(both.is_error, Some(true));
    }

    #[tokio::test]
    async fn percentage_fees_are_accepted_over_the_wire() {
        let client = connect().await;

        let result = call(
            &client,
            json!({
                "trade": { "buy_price_gp": 0, "sell_price_gp": 1_000, "quantity": 10 },
                "fees": { "kind": "percent_of_sale", "rate_basis_points": 200, "cap_per_unit_gp": 5 }
            }),
        )
        .await;

        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["fees_gp"], 50);
    }

    #[tokio::test]
    async fn invalid_requests_are_tool_errors_not_results() {
        let client = connect().await;
        let valid_trade = json!({ "buy_price_gp": 1, "sell_price_gp": 2, "quantity": 1 });

        for arguments in [
            // Missing fees: no fee rule is assumed.
            json!({ "trade": valid_trade }),
            // Missing price: never read as zero.
            json!({ "trade": { "sell_price_gp": 2, "quantity": 1 }, "fees": { "kind": "none" } }),
            // Negative price.
            json!({ "trade": { "buy_price_gp": -1, "sell_price_gp": 2, "quantity": 1 },
                    "fees": { "kind": "none" } }),
            // Unknown fee kind.
            json!({ "trade": valid_trade, "fees": { "kind": "standard_tax" } }),
            // Zero quantity, rate above 100%, and an out-of-range price.
            json!({ "trade": { "buy_price_gp": 1, "sell_price_gp": 2, "quantity": 0 },
                    "fees": { "kind": "none" } }),
            json!({ "trade": valid_trade,
                    "fees": { "kind": "percent_of_sale", "rate_basis_points": 10_001 } }),
            json!({ "trade": { "buy_price_gp": 9_007_199_254_740_992_u64, "sell_price_gp": 2,
                               "quantity": 1 }, "fees": { "kind": "none" } }),
        ] {
            let result = call(&client, arguments.clone()).await;
            assert_eq!(result.is_error, Some(true), "{arguments}: {result:?}");
        }
    }
}
