# `calculate_profit`

A deterministic, read-only calculator for a proposed trade or processing recipe. It makes no network
request and uses no market data: you supply the prices, the quantity and the fee
policy, and it returns the arithmetic using the same evaluator as the rest of the
crate (`profit::evaluate`). It is not a forecast.

## Input

| Field | Type | Notes |
| --- | --- | --- |
| `trade.buy_price_gp` | integer | Price paid per item, whole GP. `0` means a price of zero. |
| `trade.sell_price_gp` | integer | Price received per item before fees, whole GP. |
| `trade.quantity` | integer | Items bought and sold, at least 1. |
| `recipe.inputs[]` | objects | Items consumed, 1 to 50 lines of `unit_price_gp` and `quantity` (at least 1). Priced as acquired. |
| `recipe.outputs[]` | objects | Items produced and sold, 1 to 50 lines of `unit_price_gp` and `quantity`. The fee policy applies to each line. |
| `fees` | object | **Required.** No fee rule is assumed. |

Give exactly one of `trade` and `recipe`. Every field of the chosen form is
required. A missing or negative price is an error, never zero. An empty recipe side
is an error, and a recipe with one input and one output line gives the same result
as the equivalent trade.

A recipe input you already own still has an opportunity cost (what you could sell it
for). Price it deliberately: `0` means you value it at zero. This tool does not
choose for you. Outputs with different fee treatment (for example exempt items)
need separate calls.

`fees.kind` is one of:

- `unknown`: the fee rules are not known. `fees_gp`, `net_profit_gp` and
  `roi_basis_points` come back `null`; the gross figures remain.
- `none`: asserts that selling incurs no fee. On Hardcore Main Worlds the
  maintainer observed in game that the Grand Exchange charges no fee (issue #14,
  2026-11-26). That is an observation, not a documented rule, and nothing applies
  it for you: say `none` explicitly.
- `percent_of_sale` with `rate_basis_points` (0 to 10000; 100 bp = 1%, so 200 is
  2%) and an optional `cap_per_unit_gp`: a percentage of each unit's sale price,
  **rounded down per unit**, then capped per unit, then multiplied by the
  quantity. No standard-world tax is ever applied implicitly.

## Output

All amounts are whole GP as signed integers, so a loss is negative.

| Field | Meaning |
| --- | --- |
| `acquisition_cost_gp` | `buy_price_gp` times `quantity`. |
| `gross_revenue_gp` | `sell_price_gp` times `quantity`. |
| `gross_profit_gp` | Revenue minus cost. |
| `fees_gp` | Total fees, or `null` if fees are unknown. |
| `net_profit_gp` | Gross profit minus fees, or `null` if fees are unknown. |
| `roi_basis_points` | Net profit over cost in basis points (100 bp = 1%), truncated toward zero. `null` if fees are unknown or the cost is zero (undefined, not 0). |
| `fees` | The fee policy that was applied. |
| `note` | What the result does and does not mean. |

## Integer range

JSON numbers above 2^53 − 1 (9,007,199,254,740,991) lose precision in many
clients. The tool rejects any input integer above that, and any result (including
ROI) outside ±(2^53 − 1), with an explicit error instead of returning a rounded
number. A product that overflows 64-bit arithmetic is reported as an overflow
error. Realistic Grand Exchange values are far below these limits.

## Errors

Tool errors with fixed text and no echoed values: neither or both of `trade` and
`recipe`, more than 50 lines on a recipe side, an empty recipe side, an out-of-range
input, a zero quantity, a fee rate above 10000 basis points, an overflow, or a result that
cannot be represented exactly.

## Using it with the market tools

1. `item_search` finds the item id.
2. `item_prices` returns the last observed instant-buy and instant-sell trades with
   per-side freshness and a `crossed` flag.
3. Choose your own proposed passive buy and sell prices and pass them to
   `calculate_profit` with an explicit fee policy.

The last instant-sell and instant-buy trades are observations, not guaranteed fill
prices. A spread says nothing about fill rate, how long an offer takes, or GP per
hour, and a margin computed from a stale or crossed side is not reliable. This tool
only does the arithmetic on the prices you give it.

## Scope

Trades and recipes with explicit prices. This tool does not look up recipes, item
ids, prices or Grand Exchange limits, and does not estimate fill time or GP per hour.
