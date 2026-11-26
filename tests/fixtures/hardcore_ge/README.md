# Hardcore GE fixtures

Small hand-trimmed bodies shaped like `https://prices.runescape.wiki/api/v2/osrs-hc/` responses (see `docs/investigations/hardcore-ge-data-source.md`). They are not data dumps.

| File | Provenance |
| --- | --- |
| `latest.json` | Item 4151 entry copied from an observed 2026-11-26 response; item 2 and item 99 are synthetic, added to cover a one-sided low price and a side that was never seen. |
| `five_minute.json` | Items 2, 30 and 40 copied from an observed response; item 41 is synthetic and has fractional averages on both sides. |
| `timeseries_24h.json` | Four data points copied from an observed response, with a synthetic two-sided point. Header fields are as observed. |
| `mapping.json` | Synthetic, shaped from the documented field list. Observed bodies were truncated, so which fields may be absent is unverified. |

The schema these fixtures follow was checked against the live API on 2026-11-26 (field names, types, nullable sides, the `1h` top-level `timestamp`, optional `mapping` fields); the `mapping.json` optional-field set matches what was observed (`limit`, `value`, `highalch`, `lowalch` can be absent). The synthetic entries remain synthetic. The single verification and fixture-refresh procedure is "Local verification procedure" in `docs/investigations/hardcore-ge-data-source.md` (script: `scripts/verify_hardcore_api.py`); do not add a second one here. Replace these with trimmed real captures once a maintainer has run it.
