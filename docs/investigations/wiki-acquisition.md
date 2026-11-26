# Investigation: OSRS Wiki acquisition and licensing

- Status: Proposed. Awaiting maintainer decision and an unresolved permission question (see [Open questions](#open-questions)).
- Date: 2026-11-26
- Related issues: #6 (this investigation), #16 (retrieval evaluation dataset), #8 (project licence)

## Summary

Recommendation, in one paragraph: do **not** start acquiring Wiki content yet. The Wiki text is licensed CC BY-NC-SA 3.0 with additional terms, and the host's Terms of Use prohibit automated use of its sites without prior consent from the operator. No public database dump was found. Before building any ingestion, the maintainer should ask the Wiki operator (Weird Gloop) for consent to a small, serial, identified MediaWiki API client and whether dumps exist. Until then, build retrieval against tiny hand-written fixtures and the evaluation dataset from #16. If consent is given, fetch on demand and refresh incrementally through the MediaWiki API into an ignored local directory, never committed, indexed section by section with revision provenance.

## How this was investigated, and what is not verified

This investigation was done from a sandboxed environment whose network policy blocks `oldschool.runescape.wiki`, `prices.runescape.wiki`, `meta.weirdgloop.org` and most other hosts. Pages were read through a web-fetch tool that summarises pages with a model and honours `robots.txt`. Consequences:

- Quotes below are short and were extracted by that tool; they were not read directly from a browser. Treat them as leads to re-check before relying on them for a legal decision.
- `api.php`, `Special:Version` and other `Special:` pages could not be fetched, because the tool honours `robots.txt` (which disallows them for generic agents; see below). **Installed extensions, API parameters, API limits and the availability of `maxlag`, `action=parse` section output or structured-data queries on this wiki are therefore unverified.** The MediaWiki behaviours described are standard documented MediaWiki behaviour, not observed on this wiki.
- No Wiki dump, mirror or live API response was downloaded or inspected. Wiki size (page count, bytes) is **not measured**; figures must be taken from a first permitted sample.
- Observation date for everything below: 2026-11-26. Page dates quoted are as reported by the fetch tool.

## Sources consulted

| Source | What it established |
| --- | --- |
| [RuneScape:Copyrights](https://oldschool.runescape.wiki/w/RuneScape:Copyrights) (soft redirect, revision 14863039, modified 2025-03-16) to [meta.weirdgloop.org Licensing](https://meta.weirdgloop.org/w/Licensing) (revision 1299, edited 2026-09-07) | Licence, attribution, share-alike, non-text media |
| [weirdgloop.org/terms](https://weirdgloop.org/terms) (updated 2025-01-31) | Conduct rules, including on automated use |
| [oldschool.runescape.wiki/robots.txt](https://oldschool.runescape.wiki/robots.txt) | Crawler rules |
| [RuneScape:Real-time Prices](https://oldschool.runescape.wiki/w/RuneScape:Real-time_Prices) (revision 15397697, modified 2026-11-09) | The only Wiki-hosted API guidance found: User-Agent and rate expectations for the price API |
| [Forum:API and terms of use](https://oldschool.runescape.wiki/w/Forum:API_and_terms_of_use) (archived 2018 thread) | Informal staff answer about the MediaWiki API |
| [MediaWiki API:Etiquette](https://www.mediawiki.org/wiki/API:Etiquette), [API:Query](https://www.mediawiki.org/wiki/API:Query) | Generic MediaWiki client behaviour |
| PyPI and GitHub pages for `osrsbox`, `osrsreboxed`, `osrsbox-db`, `runeql-data` | Community datasets |

## Licensing findings

### Wiki text

- Footer and Licensing page: content is "licensed under CC BY-NC-SA 3.0; additional terms apply". The Weird Gloop Licensing page says this holds "on the majority of our wikis" and tells readers to confirm in each wiki's footer. In its table the row for the Old School wiki has **no licence or additional-terms entry**, and a pre-fork-date BY-SA rule is listed only for the RuneScape Wiki. The exact licence history of the OSRS Wiki is therefore not confirmed by that page.
- **NonCommercial:** the page does not define it and gives no exception for bots or research. Whether a free, local, personal advisory tool qualifies is a legal question for the maintainer, not settled here.
- **Attribution:** reusers must include at least one of a hyperlink or URL to the page, a link to an alternative stable copy, or a list of authors.
- **ShareAlike:** modifications or additions to re-used pages must be licensed under the same or a compatible licence. Section extracts, summaries and derived indexes are plausibly adaptations; this should be assumed until advised otherwise.
- **Non-text files:** the Licensing page states non-text media must not be assumed to share the text licence, and each media page states its own. Images are out of scope for this project. Jagex owns the underlying game assets and trademarks (the footer states RuneScape and RuneScape Old School are trademarks of Jagex used with permission).

### Automated access

- **Terms of Use (weirdgloop.org/terms)** list under conduct: "Attempting to use our Sites in an automated fashion without prior consent from the Company". This is the most important finding. A scraper or API client falls under it unless consent has been given, whatever the content licence permits.
- **robots.txt** for generic agents disallows `/*api.php`, `/w/Special:` (including `Special:Export`), `/w/Bucket:`, `/*rest.php/`, and many query-string forms (`?action=`, `?oldid=`, `?search=` and others). `/w/Title` article paths are not disallowed. It names a large number of bots as fully disallowed, including AI crawlers (for example GPTBot and ClaudeBot). It sets no crawl delay. Our client would not be one of the named bots, but the intent is clear: bulk or AI-oriented crawling is unwelcome without arrangement.
- **Price API guidance** (Real-time Prices page): "This is the only thing we ask!" is a descriptive User-Agent with optional contact details; several default agents (curl, python-requests, Java and others) are pre-emptively blocked; endpoints are not explicitly rate limited, but heavy sustained use may be limited, and real-time pipelines should first speak to the operators. This guidance is specific to the price API (`prices.runescape.wiki`) and should not be assumed to extend to `api.php`.
- **2018 forum answer** (informal, archived): no API-specific terms; the general Terms of Use apply; a custom User-Agent is preferred; be reasonable on request rate. The respondent said written guidance would follow; none was found.

Taken together, the content licence and API/bot rules point in different directions, and the written rules do not clearly cover a small, polite, identified client. Ask rather than assume.

### Distinguishing the layers

| Layer | Licence or terms | Notes |
| --- | --- | --- |
| This repository's application code | To be decided in #8 | Independent of Wiki text. Should not embed Wiki text. |
| Wiki prose, tables and infobox values | CC BY-NC-SA 3.0, additional terms apply | Local private index plausible; redistribution needs attribution and share-alike. |
| Wiki images and other media | Per-file | Not used. |
| Game assets, names, trademarks | Jagex | Not covered by the Wiki licence. |
| Community datasets (`osrsbox` family) | Code GPL-3.0; Wiki-derived data reported as CC BY-NC-SA 3.0 | Incompatibility between GPL code and NC data in one distribution is possible; do not bundle without review. |
| Price API data | Separate guidance; Jagex-sourced since 2026-09-30 per the Real-time Prices page | Covered by #13, not this issue. |

Practical consequence for #8: choosing an application licence does not license any Wiki-derived data shipped with it. Keep Wiki-derived indexes out of the repository and out of release artefacts unless the maintainer decides otherwise.

When Wiki content is sent to a hosted LLM through MCP, the model provider receives the text. Whether that counts as redistribution, and whether the NC term affects it, is an open question below.

## Options compared

| Option | Content and coverage | Availability and maintenance (verified) | Licensing and rules | Cost and effort | Assessment |
| --- | --- | --- | --- | --- | --- |
| **A. MediaWiki API, on demand and incremental refresh** | Whole Wiki: wikitext, rendered HTML, redirects, revision ids and timestamps, section lists. Template and module output is available through rendered output. | Standard MediaWiki feature; the Wiki reports MediaWiki 1.45.3. Endpoint behaviour on this wiki **not verified** (could not fetch `api.php`). | `robots.txt` disallows `api.php` for generic agents; Terms require prior consent for automated use. Etiquette: identified User-Agent, serial requests, batching titles with `\|`, `maxlag` for non-interactive jobs. | Low engineering effort. Cost is the permission step and polite request pacing. Initial population volume unmeasured. | Best technical fit if consent is obtained. |
| **B. Official database dump** | Would be complete, including history if full dumps. | **No dump found.** Weird Gloop Meta `Data_dumps` and `Dumps` pages returned permanent errors, the Meta main page links no dumps, `dumps.weirdgloop.org` could not be checked. Existence is **unknown**, not disproven. | Would still carry the content licence. Avoids load on live site. | Unknown size; needs wikitext parser or template expansion, since raw wikitext lacks expanded templates. | Ask the operator. Preferred if it exists. |
| **C. `Special:Export` or HTML page scraping** | Per page. HTML paths (`/w/Title`) are not disallowed. | Available in principle. | Scraping falls under the same automated-use consent; `Special:` is disallowed. | Fragile HTML parsing, more load per page than API batching. | Reject as a bulk method. Acceptable only for a handful of manual spot checks. |
| **D. Community datasets (`osrsbox`, `osrsreboxed`, `runeql-data`)** | Structured item, monster and prayer data (stats, examine text, Wiki names and URLs). No article prose, so they cannot serve section retrieval. | `osrsbox` latest PyPI release 2.2.3 (2021-09-30, per the PyPI page); `osrsreboxed` 2.3.33 (2025-01-07); `osrsbox-db` shows no release information and unverified recent activity; `runeql-data` is a downstream staging repo with no licence shown. Update processes are manual or semi-manual, and skill requirements are hand-maintained. | Code GPL-3.0; Wiki-derived data CC BY-NC-SA 3.0. | Cheap to read, but staleness and licence mixing. | Not a retrieval source. Possibly a cross-check for item identifiers later; decide in the structured-data work, not here. |
| **E. Third-party Wiki mirrors or archives** | Not assessed. | No mirror was verified. Not looked up in depth; an unofficial mirror's freshness and terms would need checking. | Mirrors may not honour attribution or may be stale. | Unknown. | Not recommended without a verified, maintained mirror. |
| **F. Game cache** | Authoritative structured game data, no prose. | Out of scope for this issue. | Jagex terms apply. | Significant. | Separate decision for the structured-data workstream. |

## Recommendation

1. **Ask first.** The maintainer contacts Weird Gloop (the `#api-discussion` channel named on the Real-time Prices page is the stated route for pipeline questions) with the request below.
2. **Preferred outcome: a dump.** If one exists, use it for the initial local corpus and refresh from it.
3. **Otherwise, with consent: option A**, with these constraints written into the client when it is built:
   - Descriptive User-Agent naming the project and a contact route chosen by the maintainer, not hard-coded personal details.
   - Serial requests only, batched titles, `maxlag` set, generous delay, a hard daily request budget, and an off switch.
   - Fetch is user-initiated or scheduled at low frequency, never at query time from an LLM request.
   - Responses are bounded in size; errors back off.
   - Refresh incrementally: compare stored revision ids against `recentchanges` or a revision-id query, and fetch only changed pages.
4. **Local storage only.** Raw responses and indexes live in the already-ignored `/wiki-dump/` and `/indexes/` directories (see `.gitignore`). Nothing from the Wiki is committed, and tests use small synthetic fixtures.
5. **No bundled Wiki data in releases** until the share-alike and non-commercial questions are answered.
6. **Interim work that does not need the Wiki:** implement and test the chunker and index against synthetic fixtures, and use the dataset from #16 for the evaluation.

Draft request to the operator (for the maintainer to adapt and send; this thread does not contact anyone):

> I'm building a free, local, read-only Old School RuneScape advisory tool that lets an LLM look up Wiki sections. I would like to fetch pages through the MediaWiki API (serial, batched, identified User-Agent, `maxlag`, a daily budget, incremental refresh by revision id) and keep a private local index with a link back to each source page and revision. Is that permitted? Is there a database dump I should use instead? Is there anything about the non-commercial term that I should know about, given that the retrieved sections are shown to an LLM chosen by the user?

## Proposed section-aware indexing (design only, nothing is built)

**Unit of retrieval:** a section, identified by its heading path within a page, not a whole page and not an arbitrary fixed-size window.

**Fields stored per section:**

| Field | Purpose |
| --- | --- |
| `page_title`, `page_id` | Stable identity, with redirects stored as aliases of the target title |
| `section_path` (for example `["Strategies", "Safespotting"]`) and `section_anchor` | Human-readable and linkable section identity |
| `revision_id`, `revision_timestamp` | Provenance of exactly what was indexed |
| `fetched_at` | Freshness. Missing or stale values stay distinguishable, per the project rules |
| `source_url` | Page URL with the section anchor, and a revision permalink, for attribution |
| `licence` | The licence string observed in the footer at fetch time |
| `text` | Plain text of the section |
| `kind` | Prose, table or list, so tables are not flattened into noise |

**Content form:** rendered output, converted to plain text per section, is preferred to raw wikitext. Wikitext contains unexpanded templates and module calls (infoboxes, drop tables, recipes), which hold much of the useful content. Keep the revision id so the rendered text can be reproduced. Rendered tables should be kept as structured rows, with only a text form indexed. Extracting structured facts (shops, requirements, drops) is a separate workstream and should not be derived from retrieval text.

**Index:** start with keyword retrieval (title and heading boost plus full text, for example SQLite FTS5, noted as an option only because the README lists SQLite as a proposed direction). Add embeddings only if the evaluation from #16 shows keyword retrieval misses queries that matter, as the README principle says. Redirects and page titles give exact-name matching for free.

**Tool-facing behaviour (for later):** return a bounded number of sections with their provenance, and treat all returned text as untrusted data, per AGENTS.md.

**Coverage questions to measure on a permitted sample:** pages per namespace, template-heavy pages, redirect counts, how tables and collapsible content render, and how update latency compares with game updates.

## Sample retrieval evaluation

A small evaluation is proposed rather than run, because no retrieval engine exists. [#16](https://github.com/whcas/OSRS-MCP/issues/16) defines the dataset, metrics and procedure. For this investigation the evidence is limited to:

- A handful of article titles checked for existence through the fetch tool, recorded in the #16 dataset as verified with their observation date.
- The acquisition options above are **not** benchmarked.

## Cost and trade-off summary

- **Permission is the real cost.** Engineering for any option is small. The unknown is whether the operator consents, and on what terms.
- **API route** gives freshness and provenance at the price of load on the operator and an ongoing politeness burden. **Dump route** (if it exists) is cheaper for everyone but may lag and needs template handling.
- **Staleness risk:** game updates change values; revision ids and fetch timestamps let the advisor flag old text rather than trust it.
- **Storage:** unmeasured. Likely small enough for a local database, but this is an assumption.

## Open questions

1. **Consent:** does Weird Gloop consent to a small identified API client, and under what limits? Is a dump available?
2. **NonCommercial:** does a free personal-use tool satisfy "non-commercial", including if it is later published as open source?
3. **LLM transmission:** is sending retrieved sections to a hosted model redistribution, adaptation or neither under BY-NC-SA 3.0?
4. **ShareAlike scope:** is a derived index or a bounded section extract an adaptation that must itself be CC BY-NC-SA?
5. **OSRS Wiki licence row:** the Weird Gloop table has no entry for this wiki. Confirm the licence and any additional terms from the live footer.
6. **Attribution form:** exact wording the operator wants alongside returned sections.
7. **robots.txt and AI crawlers:** confirm that a user-run local client with an honest User-Agent is acceptable, given named AI bots are disallowed.
8. **Unverified technical points:** API availability and limits, installed extensions (the `/w/Bucket:` path in `robots.txt` and a Bucket link on Weird Gloop Meta suggest the Bucket structured-data extension is in use, unconfirmed), `maxlag` support, rendered-table fidelity.

## Verification and revisit criteria

- The recommendation is revisited when the operator answers question 1, or if a dump or mirror is found.
- Before any ingestion code is written, record the consent decision (and any limits) in an ADR under `docs/adr/`.
- Re-check the licence, Terms of Use, `robots.txt` and the Real-time Prices page at that time, since they change; quotes here are dated 2026-11-26.
