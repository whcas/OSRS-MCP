# Market tools

Read-only tools over Hardcore Grand Exchange data. They query only the Hardcore
endpoint set (`osrs-hc`) of the prices API, never fall back to standard-world
data, and report where and when each result was retrieved. Failures are tool
errors with fixed text; response bodies are never echoed. There is no retry,
cache or polling.

## `item_search`

Finds items in the Hardcore item mapping by exact id or by name. One request to
the mapping endpoint per valid call; invalid input makes no request.

### Input

Give exactly one of `item_id` or `name`.

| Field | Type | Notes |
| --- | --- | --- |
| `item_id` | integer | Exact item id. |
| `name` | string | 2 to 100 characters after trimming. Case-insensitive. |
| `limit` | integer | 1 to 25, default 10. |

### Output

| Field | Meaning |
| --- | --- |
| `matches[]` | Best match first. Empty when nothing matched (not an error). |
| `matches[].item_id`, `name` | The item, as reported by the source. Names are untrusted data. |
| `matches[].match_kind` | `id`, `exact_name`, `prefix` or `substring`. |
| `matches[].members`, `buy_limit`, `value`, `low_alchemy_gp`, `high_alchemy_gp` | Source metadata. **Absent** when the source omitted it; never defaulted to zero or false. `buy_limit` is the reported Grand Exchange limit with no time period implied; the `*_gp` and `value` fields are whole GP. |
| `total_matches` | Matches before `limit` was applied. |
| `truncated` | `limit` cut off further matches. |
| `ambiguous` | More than one item shares the best match kind, so the first match alone does not identify one item. |
| `source` | `world_type` (always `hardcore`), `endpoint`, `source_url`, and `retrieved_at_unix_seconds`. |
| `note` | Reminder that a match is not proof of trading. |

Ranking is exact name, then prefix, then substring. Ties are broken by lowercased
name, then name, then item id, so the order does not depend on the source order.
An exact id lookup returns at most one match. Distinct ids can share a name.

### Example

Request `{"name": "steel cannonball", "limit": 3}`. In a synthetic mapping
that contains a single `Steel cannonball` entry with item id `2`, the tool lists
that entry first with `match_kind: "exact_name"` and `ambiguous: false`. Searching
for `cannonball` can return several candidates, so check the names and ids before
choosing one. Names and matches depend on the current mapping.

### Limitations

- `retrieved_at_unix_seconds` is the time this server finished retrieving and
  parsing the response, not an exact arrival time and not a trade time.
- A mapping match shows that an item exists in the Hardcore item mapping. It does
  not show that the item is traded, has a price, or is available on Hardcore
  worlds in practice.
- Search is by name text only. There is no fuzzy or alias matching (for example,
  common abbreviations), and no caching: each call fetches the mapping again.
- A mapping with repeated item ids returns a malformed-response error.
- A name over 256 UTF-8 bytes returns a tool error if its match was selected
  for the response. Other lookups can still use the same mapping. Names in
  successful results are returned in full.

## `item_prices`

Latest observed trades for one item from the `latest` endpoint. One request per
valid call; invalid input makes no request. Use `item_search` first to find an
item id.

### Input

| Field | Type | Notes |
| --- | --- | --- |
| `item_id` | integer | Required. |
| `max_age_seconds` | integer | Optional. How old a side may be and still be `fresh`. 0 to 2,592,000 (30 days), default 3,600. Inclusive: age equal to the limit is fresh, and 0 is valid (only a trade at exactly the retrieval time is fresh). |

### Output

| Field | Meaning |
| --- | --- |
| `item_id` | The requested id. |
| `present_in_latest` | The item appeared in the response. This does not show it is listed or tradable. |
| `high` | Last observed instant-buy trade, or `null` if no observation was returned. |
| `low` | Last observed instant-sell trade, or `null` if no observation was returned. |
| `high`/`low` `.price_gp` | Whole GP. A real `0` is reported as `0`. |
| `high`/`low` `.observed_at_unix_seconds` | When the trade was observed (seconds since the Unix epoch, UTC). |
| `high`/`low` `.freshness` | `fresh`, `stale` or `future`, judged **per side** against `max_age_seconds`. |
| `high`/`low` `.age_seconds` | Age in whole seconds at the retrieval time (`fresh` and `stale`). |
| `high`/`low` `.ahead_seconds` | How far after the retrieval time the trade is dated (`future` only). |
| `crossed` | Only when both sides are present: `low` is above `high`. Both prices are unchanged. |
| `max_age_seconds` | The limit applied. |
| `evaluated_at_unix_seconds` | The reference time for ages: the retrieval time. |
| `source` | As for `item_search`, with `endpoint` `latest`. |
| `note` | Reminder that observations are not executable offers. |

Missing and stale data are results, not errors. An omitted item
(`present_in_latest: false`), a side with no observation (`null`), a stale side and
a future-dated side are all reported distinctly. Transport, HTTP, malformed and
oversized responses are tool errors with fixed text. A stale observation is
still returned with its age so the caller can decide how much to trust it.

### Reading the result

- `high` and `low` are the last trades observed on each side. They are not
  guaranteed executable offers, and a trade at that price may not be available
  again.
- Sides can be crossed (`low` above `high`) and one side can be much older than
  the other, so check `crossed` and each side's `freshness` before computing a
  spread. A margin built from a stale or crossed side is not a reliable
  estimate.
- Ages are measured at the retrieval time, not at the moment you read the result.

### Limitations

- Prices only: no volume, history or averages. No caching, retry or polling; each
  call fetches the latest observations again.
- `null` means no observation was returned for that side, not that the item has
  never traded.
