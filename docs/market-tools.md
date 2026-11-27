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

### Example

Request: `{"name": "cannonball", "limit": 3}`. A response lists `Cannonball`
first as an `exact_name` match, with `ambiguous: false` because it is the only
exact match, followed by prefix and substring matches such as `Cannonball mould`.

### Limitations

- `retrieved_at_unix_seconds` is the time this server finished retrieving and
  parsing the response, not an exact arrival time and not a trade time.
- A mapping match shows that an item exists in the Hardcore item mapping. It does
  not show that the item is traded, has a price, or is available on Hardcore
  worlds in practice.
- Search is by name text only. There is no fuzzy or alias matching (for example,
  common abbreviations), and no caching: each call fetches the mapping again.
