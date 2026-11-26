# Investigation: OSRS Wiki acquisition and licensing

- Status: Proposed. The maintainer has deferred the consent request and any Wiki acquisition (2026-11-26); the permission questions remain open (see [Open questions](#open-questions)) and nothing should be fetched in bulk until they are answered.
- Date: 2026-11-26
- Related issues: #6 (this investigation), #16 (retrieval evaluation dataset), #8 (project licence)

## Summary

Recommendation, in one paragraph: do **not** start acquiring Wiki content yet. The Wiki text is licensed CC BY-NC-SA 3.0 with additional terms, and the host's Terms of Use prohibit automated use of its sites without prior consent from the operator. No public database dump was found. Before building any ingestion, the maintainer should ask the Wiki operator (Weird Gloop) for consent to a small, serial, identified MediaWiki API client and whether dumps exist. Until then, develop the chunker and index against small synthetic fixtures that carry their own hand-written ground truth; the #16 Wiki benchmark is meant for a permitted real corpus and must not be run against synthetic content. If consent is given, fetch on demand and refresh incrementally through the MediaWiki API into an ignored local directory, never committed, indexed section by section with revision provenance.

## How this was investigated, and what is not verified

This investigation was done from a sandboxed environment whose network policy blocks `oldschool.runescape.wiki`, `prices.runescape.wiki`, `meta.weirdgloop.org` and most other hosts. Pages were read through a web-fetch tool that summarises pages with a model and honours `robots.txt`. Consequences:

- Quotes below are short and were extracted by that tool; they were not read directly from a browser. Treat them as leads to re-check before relying on them for a legal decision.
- `api.php`, `Special:Version` and other `Special:` pages could not be fetched, because the tool honours `robots.txt` (which disallows them for generic agents; see below). **Installed extensions, API parameters, API limits and the availability of `maxlag`, `action=parse` section output or structured-data queries on this wiki are therefore unverified.** The MediaWiki behaviours described are standard documented MediaWiki behaviour, not observed on this wiki.
- No Wiki dump, mirror or live API response was downloaded or inspected. Wiki size (page count, bytes) is **not measured**; figures must be taken from a first permitted sample.
- Observation date for everything below: 2026-11-26. Page dates quoted are as reported by the fetch tool.

## Evidence quality

Every claim below passed through the fetch tool's model-based extraction; none was read from raw page text by the author. The table says how each material licensing and terms claim is supported, so a reviewer knows what to re-check first.

- **Quoted:** the tool returned a short phrase presented as verbatim page text. The quotation itself was not independently checked.
- **Reported:** the tool's own summary or paraphrase of the page.
- **Corroborated:** the same fact came back from more than one separate fetch.

| Claim | Evidence | Weakness |
| --- | --- | --- |
| Footer text "licensed under CC BY-NC-SA 3.0; additional terms apply" | Quoted; corroborated by four separate fetches (Copyrights, Licensing, Forum and Real-time Prices pages) | Footer on other pages not checked |
| The Weird Gloop licensing table has no licence entry for the OSRS Wiki | Reported by two separate fetches of the same page | Table reading is a summary; check the live page |
| Attribution and ShareAlike wording | Quoted from the Licensing page, one fetch | Single source |
| NonCommercial is undefined and has no bot or research exception | Reported (an absence claim) | An absence found by a summarising tool is weak evidence |
| Terms of Use prohibit automated use without prior consent | Quoted, one fetch of a page dated 2025-01-31 | Single source; the surrounding wording was not read |
| `robots.txt` rules and the list of disallowed bots | Reported from the fetched text, one fetch | Long file summarised by a model; re-read it directly |
| Real-time Prices guidance (User-Agent, rate expectations) | Quoted, one fetch | Applies to the price API only |
| 2018 forum answer | Quoted; the tool reported checking the quotes against the text | Informal and old |
| No dump found | Reported: two Meta pages returned a tool-level "client error" (cause unknown, so a missing page and a blocked fetch are indistinguishable); the Meta main page was reported to link no dumps | Weak. A dump may exist |
| Community dataset licences, release dates and contents | Reported; several points were marked as summaries by the tool, and `osrsbox-db`'s last commit date was not seen | Re-check on the repositories |

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
- **ShareAlike:** modifications or additions to re-used pages must be licensed under the same or a compatible licence. Whether section extracts, summaries and derived indexes count as adaptations that must be shared alike is **unresolved** and not concluded here. As a precaution pending advice, nothing derived from the Wiki is committed or redistributed.
- **Non-text files:** the Licensing page states non-text media must not be assumed to share the text licence, and each media page states its own. Images are out of scope for this project. Jagex owns the underlying game assets and trademarks (the footer states RuneScape and RuneScape Old School are trademarks of Jagex used with permission).

### Automated access

- **Terms of Use (weirdgloop.org/terms)** list under conduct: "Attempting to use our Sites in an automated fashion without prior consent from the Company". This is the most important finding. A scraper or API client falls under it unless consent has been given, whatever the content licence permits.
- **robots.txt** for generic agents disallows `/*api.php`, `/w/Special:` (including `Special:Export`), `/w/Bucket:`, `/*rest.php/`, and many query-string forms (`?action=`, `?oldid=`, `?search=` and others). `/w/Title` article paths are not disallowed. It names a large number of bots as fully disallowed, including AI crawlers (for example GPTBot and ClaudeBot). It sets no crawl delay. Our client would not be one of the named bots, but the file suggests that bulk or AI-oriented crawling is unwelcome without arrangement; its intent for a small identified client is not stated.
- **Price API guidance** (Real-time Prices page): "This is the only thing we ask!" is a descriptive User-Agent with optional contact details; several default agents (curl, python-requests, Java and others) are pre-emptively blocked; endpoints are not explicitly rate limited, but heavy sustained use may be limited, and real-time pipelines should first speak to the operators. This guidance is specific to the price API (`prices.runescape.wiki`) and should not be assumed to extend to `api.php`.
- **2018 forum answer** (informal, archived): no API-specific terms; the general Terms of Use apply; a custom User-Agent is preferred; be reasonable on request rate. The respondent said written guidance would follow; none was found.

Taken together, the content licence and API/bot rules point in different directions, and the written rules do not clearly cover a small, polite, identified client. Ask rather than assume.

### Distinguishing the layers

| Layer | Licence or terms | Notes |
| --- | --- | --- |
| This repository's application code | To be decided in #8 | Independent of Wiki text. Should not embed Wiki text. |
| Wiki prose, tables and infobox values | CC BY-NC-SA 3.0, additional terms apply | Whether a private local index is permitted is an open question (see [Open questions](#open-questions)). Redistribution would at least raise attribution and ShareAlike questions. |
| Wiki images and other media | Per-file | Not used. |
| Game assets, names, trademarks | Jagex | Not covered by the Wiki licence. |
| Community datasets (`osrsbox` family) | Code GPL-3.0; Wiki-derived data reported as CC BY-NC-SA 3.0 | Incompatibility between GPL code and NC data in one distribution is possible; do not bundle without review. |
| Price API data | Separate guidance; Jagex-sourced since 2026-09-30 per the Real-time Prices page | Covered by #13, not this issue. |

Relevance to #8: the application licence is a separate matter from the terms on any Wiki-derived data. The working precaution, pending the answers below, is to keep Wiki-derived indexes out of the repository and out of release artefacts unless the maintainer decides otherwise.

When Wiki content is sent to a hosted LLM through MCP, the model provider receives the text. Whether that counts as redistribution, and whether the NC term affects it, is an open question below and is not concluded here.

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
6. **Interim work that does not need the Wiki:** implement and test the chunker and index against small synthetic fixtures. A synthetic corpus needs its own hand-written ground truth (a handful of invented pages with known sections and queries) and tests mechanics such as section splitting, redirect aliases and provenance fields. Its scores say nothing about retrieval quality on real Wiki content. The #16 dataset refers to real Wiki pages and can only be run against a permitted real corpus, so it waits for the consent decision.

Draft request to the operator (for the maintainer to adapt; **not sent**: this thread contacts nobody). Each use is asked about separately so the answers do not blur together:

> I'm building a free, read-only Old School RuneScape advisory tool. The code may be published as open source; I would not publish Wiki content in the repository. The tool lets a user's chosen LLM look up Wiki sections through MCP. Hosted model providers selected by the user may therefore receive bounded excerpts of Wiki text, each with a link back to the page and revision. I'd like to ask about each of these separately:
>
> 1. **API access:** may I fetch pages through the MediaWiki API with a descriptive User-Agent, serial requests, batched titles, `maxlag`, a daily request budget and incremental refresh by revision id? Are there limits you would set?
> 2. **Dumps:** is there a database dump I should use instead?
> 3. **Local index:** may I keep a private local index of that content for my own use?
> 4. **Open-source publication:** is publishing the application code, without Wiki content, as open source acceptable, and does the non-commercial term affect how the project may be published or used?
> 5. **Excerpts sent to LLMs:** is it acceptable for bounded excerpts to be sent to hosted model providers chosen by the user?
> 6. **Attribution:** what attribution wording do you want shown with returned excerpts?

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

**Index:** start with keyword retrieval (title and heading boost plus full text, for example SQLite FTS5, noted as an option only because the README lists SQLite as a proposed direction). Add embeddings only if an evaluation on a permitted real corpus (the #16 benchmark) shows keyword retrieval misses queries that matter, as the README principle says. Redirects and page titles give exact-name matching for free.

**Tool-facing behaviour (for later):** return a bounded number of sections with their provenance, and treat all returned text as untrusted data, per AGENTS.md.

**Coverage questions to measure on a permitted sample:** pages per namespace, template-heavy pages, redirect counts, how tables and collapsible content render, and how update latency compares with game updates.

## Sample retrieval evaluation

A retrieval evaluation is proposed rather than run, because no retrieval engine exists and no permitted real corpus exists. [#16](https://github.com/whcas/OSRS-MCP/issues/16) defines the Wiki-based dataset, metrics and procedure. The two kinds of evaluation are kept apart:

- **Real Wiki benchmark (#16):** judged against real Wiki pages and sections. It can be run only against a corpus acquired with the operator's consent, and it measures retrieval quality on real content.
- **Synthetic fixtures:** invented pages with their own hand-written expected sections. They test the mechanics of the chunker and index offline, in CI, with no Wiki content. They do not measure real retrieval quality and must not reuse the #16 references.

For this investigation the evidence is limited to a handful of article titles checked for existence through the fetch tool, recorded in the #16 dataset with their observation date. The acquisition options above are **not** benchmarked.

## Cost and trade-off summary

- **Permission is the real cost.** Engineering for any option is small. The unknown is whether the operator consents, and on what terms.
- **API route** gives freshness and provenance at the price of load on the operator and an ongoing politeness burden. **Dump route** (if it exists) is cheaper for everyone but may lag and needs template handling.
- **Staleness risk:** game updates change values; revision ids and fetch timestamps let the advisor flag old text rather than trust it.
- **Storage:** unmeasured. Likely small enough for a local database, but this is an assumption.

## Open questions

1. **Consent:** does Weird Gloop consent to a small identified API client, and under what limits? Is a dump available?
2. **Private local index:** is keeping a local index for personal use permitted, and is that use "non-commercial"? This is a question for the operator or legal advice; it is not concluded here.
3. **Open-source publication:** does publishing the application code (without Wiki content) as open source affect the non-commercial or ShareAlike terms, or what is permitted for the tool's users?
4. **Excerpts sent to hosted LLMs:** may bounded excerpts be sent to hosted model providers chosen by the user, and is that redistribution, adaptation or neither under BY-NC-SA 3.0?
5. **ShareAlike scope:** is a derived index or a bounded section extract an adaptation that must itself be CC BY-NC-SA?
6. **OSRS Wiki licence row:** the Weird Gloop table has no entry for this wiki. Confirm the licence and any additional terms from the live footer.
7. **Attribution form:** exact wording the operator wants alongside returned sections.
8. **robots.txt and AI crawlers:** confirm that a user-run local client with an honest User-Agent is acceptable, given named AI bots are disallowed.
9. **Unverified technical points:** API availability and limits, installed extensions (the `/w/Bucket:` path in `robots.txt` and a Bucket link on Weird Gloop Meta suggest the Bucket structured-data extension is in use, unconfirmed), `maxlag` support, rendered-table fidelity.

## Verification and revisit criteria

- The recommendation is revisited when the operator answers question 1, or if a dump or mirror is found.
- Before any ingestion code is written, record the consent decision (and any limits) in an ADR under `docs/adr/`.
- Re-check the licence, Terms of Use, `robots.txt` and the Real-time Prices page at that time, since they change; quotes here are dated 2026-11-26.
