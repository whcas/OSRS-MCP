# Investigation: OSRS Wiki acquisition and licensing

- Status: The acquisition decision is accepted: the maintainer selected human-selected, manually exported `Special:Export` XML as the MVP input (issue #6, recorded in ADR-0002). Revised on 2026-11-26 to follow it. The legal, reuse and attribution questions are not accepted and stay open. Automated acquisition and operator contact are parked, and the licensing and permission questions remain open (see [Open questions](#open-questions)).
- Date: 2026-11-26
- Related issues: #6 (this investigation), #16 (retrieval evaluation dataset), #8 (project licence)

## Summary

**Recommendation:** use **human-selected, manually downloaded MediaWiki `Special:Export` XML files** as the MVP input, imported locally. Store pages revision by revision, split them into sections for retrieval, and keep a persistent, deduplicated list of page titles the application found missing, formatted so the maintainer can paste it into the export form. The application and its agents must not fetch Wiki pages, automate the export form, crawl, or contact the operator. See [ADR-0002](../adr/0002-manual-wiki-export-corpus.md) for the short decision record.

Manual export removes the need for an automated access mechanism, which was the main blocker found by this investigation (the host's Terms of Use prohibit automated use of its sites without prior consent). It does **not** settle reuse, attribution, ShareAlike, private indexing or the transmission of excerpts to hosted models. Those stay open, and real-corpus indexing and use should wait until the applicable terms have been verified. Automated acquisition (the MediaWiki API) and any message to the operator remain **parked**; the earlier analysis of both is kept below because it explains why.

## How this was investigated, and what is not verified

This investigation was done from a sandboxed environment whose network policy blocks `oldschool.runescape.wiki`, `prices.runescape.wiki`, `meta.weirdgloop.org` and most other hosts. Pages were read through a web-fetch tool that summarises pages with a model and honours `robots.txt`. Consequences:

- Quotes below are short and were extracted by that tool; they were not read directly from a browser. Treat them as leads to re-check before relying on them for a legal decision.
- `api.php`, `Special:Version` and other `Special:` pages could not be fetched, because the tool honours `robots.txt` (which disallows them for generic agents; see below). **Installed extensions, API parameters, API limits and the availability of `maxlag`, `action=parse` section output or structured-data queries on this wiki are therefore unverified.** The MediaWiki behaviours described are standard documented MediaWiki behaviour, not observed on this wiki.
- No Wiki dump, mirror, export file or live API response was downloaded or inspected by this session. The one export sample was supplied by the maintainer and is described here only from the maintainer's report. Wiki size (page count, bytes) is **not measured**.
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
| Real manual export sample: 8 pages, 170,106 bytes, export namespace 0.11, generator 1.45.3; page ids, namespaces, revision ids and timestamps, hashes, wikitext, explicit redirects whose targets are absent, nested templates, inline tables and unexpanded dynamically generated tables | **Maintainer-reported** in issue #6. This session has not seen the file, which must stay out of Git | One current-revision sample; not a guarantee of the schema for every page or export option |
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

Taken together, the content licence and API/bot rules point in different directions, and the written rules do not clearly cover a small, polite, identified client. Rather than assume, the MVP avoids automated access altogether (option G); asking the operator is parked.

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
| **A. MediaWiki API, on demand and incremental refresh** | Whole Wiki: wikitext, rendered HTML, redirects, revision ids and timestamps, section lists. Template and module output is available through rendered output. | Standard MediaWiki feature; the Wiki reports MediaWiki 1.45.3. Endpoint behaviour on this wiki **not verified** (could not fetch `api.php`). | `robots.txt` disallows `api.php` for generic agents; Terms require prior consent for automated use. Etiquette: identified User-Agent, serial requests, batching titles with `\|`, `maxlag` for non-interactive jobs. | Low engineering effort. Cost is the permission step and polite request pacing. Initial population volume unmeasured. | Best technical fit if consent were obtained. **Parked.** |
| **B. Official database dump** | Would be complete, including history if full dumps. | **No dump found.** Weird Gloop Meta `Data_dumps` and `Dumps` pages returned permanent errors, the Meta main page links no dumps, `dumps.weirdgloop.org` could not be checked. Existence is **unknown**, not disproven. | Would still carry the content licence. Avoids load on live site. | Unknown size; needs wikitext parser or template expansion, since raw wikitext lacks expanded templates. | Parked with operator contact. Would be preferred if it exists. |
| **C. Automated `Special:Export` or HTML page scraping** | Per page. HTML paths (`/w/Title`) are not disallowed. | Available in principle. | Automating either falls under the same automated-use consent; `robots.txt` disallows `Special:` for generic agents. | Fragile HTML parsing, more load per page than API batching. | Reject. The application and its agents must not do this. The human-driven form is option G. |
| **D. Community datasets (`osrsbox`, `osrsreboxed`, `runeql-data`)** | Structured item, monster and prayer data (stats, examine text, Wiki names and URLs). No article prose, so they cannot serve section retrieval. | `osrsbox` latest PyPI release 2.2.3 (2021-09-30, per the PyPI page); `osrsreboxed` 2.3.33 (2025-01-07); `osrsbox-db` shows no release information and unverified recent activity; `runeql-data` is a downstream staging repo with no licence shown. Update processes are manual or semi-manual, and skill requirements are hand-maintained. | Code GPL-3.0; Wiki-derived data CC BY-NC-SA 3.0. | Cheap to read, but staleness and licence mixing. | Not a retrieval source. Possibly a cross-check for item identifiers later; decide in the structured-data work, not here. |
| **E. Third-party Wiki mirrors or archives** | Not assessed. | No mirror was verified. Not looked up in depth; an unofficial mirror's freshness and terms would need checking. | Mirrors may not honour attribution or may be stale. | Unknown. | Not recommended without a verified, maintained mirror. |
| **F. Game cache** | Authoritative structured game data, no prose. | Out of scope for this issue. | Jagex terms apply. | Significant. | Separate decision for the structured-data workstream. |
| **G. Manual `Special:Export` XML, chosen and downloaded by a human (selected MVP)** | Only the pages the human selects, current revision by default. The maintainer-reported sample shows page and revision metadata, raw wikitext and explicit redirects. Templates, redirect targets and linked pages are present only if they were also selected, and dynamically generated tables stay unexpanded. | Export UI is at `https://oldschool.runescape.wiki/w/Special:Export` (maintainer-supplied). Behaviour is known from one maintainer-supplied sample only. | No automated access, so the automated-use clause is not engaged by the application. Whether a human using the export form is covered is not concluded here. Manual export does **not** settle reuse, attribution, ShareAlike, private indexing or excerpts sent to hosted models. | Low engineering effort (an offline XML importer). Ongoing human effort: select pages, download, drop files in an ignored folder. A demand-driven queue keeps that effort small. | **Recommended for the MVP**, with the licensing questions kept open. |

## Recommendation

The decision itself is recorded in [ADR-0002](../adr/0002-manual-wiki-export-corpus.md): the corpus comes from human-selected, manually downloaded `Special:Export` XML, current revision by default, kept in an ignored local directory (`/wiki-dump/` or `/data/` in `.gitignore`) and imported offline. The application and its agents never fetch Wiki pages, automate the export form, crawl or contact the operator, and nothing from an export is committed. What follows adds only the reasoning and conditions that are not in the ADR.

1. **Why this input.** It avoids engaging the automated-use clause, needs only an offline importer, and the sample already shows what the corpus looks like (page and revision metadata, raw wikitext, explicit redirects, nested templates, unexpanded generated tables). The cost is human effort and incomplete, snapshot coverage.
2. **Missing material is visible.** Missing redirect targets, templates and pages are reported, never hidden or silently resolved, and raw wikitext is kept rather than rendered. A `template_only` section in an already imported article is a rendering limitation, so exporting that article again does not fix it.
3. **Requests stay explicit.** Titles reach the paste-ready request list only through an explicit local action, never from search, retrieval or import; an import may mark existing requests satisfied. The behaviour is specified in #35 and #36.
4. **Provenance.** Every returned section carries page title, section path, page and revision ids, timestamp and a source link, so attribution is possible later.
5. **Before using a real corpus:** verify the applicable terms (reuse, attribution, ShareAlike, private indexing and excerpts sent to hosted models), and get approval before any new external transfer or redistribution. Manual export is not blanket consent for any of these.
6. **Synthetic work proceeds independently.** The importer, section splitter, storage and retrieval can be built and tested against small **synthetic** XML that mimics the observed structure, with its own hand-written ground truth. Synthetic scores say nothing about real Wiki retrieval quality. The #16 benchmark refers to real pages and is run against a real corpus only on an opt-in basis, subject to the permissions above.
7. **Parked:** automated acquisition through the MediaWiki API, requests for dumps, and any message to Weird Gloop. The draft below is kept for the maintainer's use and has **not** been sent. Do not block the MVP on these.

Implementation is tracked separately: #32 (offline XML importer), #33 (section-aware wikitext preparation), #34 (local storage and keyword retrieval), #35 (demand-driven manual request queue), #36 (MCP tools, read-only by default) and #37 (coverage-aware retrieval evaluation). This pull request stays documentation only.

### Parked: draft request to the operator

This text is for the maintainer if they decide to contact the operator later. **It has not been sent and nothing in this project sends it.** Each use is asked about separately so the answers do not blur together:

> I'm building a free, read-only Old School RuneScape advisory tool. The code may be published as open source; I would not publish Wiki content in the repository. The tool lets a user's chosen LLM look up Wiki sections through MCP. Hosted model providers selected by the user may therefore receive bounded excerpts of Wiki text, each with a link back to the page and revision. I'd like to ask about each of these separately:
>
> 1. **API access:** may I fetch pages through the MediaWiki API with a descriptive User-Agent, serial requests, batched titles, `maxlag`, a daily request budget and incremental refresh by revision id? Are there limits you would set?
> 2. **Dumps:** is there a database dump I should use instead?
> 3. **Local index:** may I keep a private local index of that content for my own use?
> 4. **Open-source publication:** is publishing the application code, without Wiki content, as open source acceptable, and does the non-commercial term affect how the project may be published or used?
> 5. **Excerpts sent to LLMs:** is it acceptable for bounded excerpts to be sent to hosted model providers chosen by the user?
> 6. **Attribution:** what attribution wording do you want shown with returned excerpts?

### Parked: constraints for a future automated client

If automated access is ever approved, these constraints would apply: a descriptive User-Agent with a contact route chosen by the maintainer, serial requests, batched titles, `maxlag`, a generous delay, a hard daily request budget, an off switch, bounded responses with backoff on errors, incremental refresh by comparing revision ids, and never fetching at query time from an LLM request.

## Indexing observations from the sample

The design belongs in #33 (section preparation) and #34 (storage and search); only what the sample and the project rules imply is recorded here.

- **Unit of retrieval:** a section, identified by its heading path within a page.
- **Provenance per section:** page title, page id, namespace, revision id and timestamp, content hash, import time, a source link and an attribution-ready permalink. Missing or stale values stay distinguishable, per the project rules.
- **Content form:** raw wikitext split at headings, with tables and unexpanded template calls kept as found and marked rather than presented as rendered. Extracting structured facts (shops, requirements, drops) is a separate workstream.
- **Missing material is a first-class state:** an unimported redirect target, template or page is reported to callers and can be requested explicitly; retrieval never implies that absent knowledge does not exist.
- **Request list:** persistent, deduplicated by normalised title, one title per line for the export form. The form's exact input format is unverified and should be confirmed by the maintainer.
- **Index:** start with keyword retrieval (SQLite FTS5 is an option, noted because the README lists SQLite as a proposed direction). Add embeddings only if an evaluation on a permitted real corpus shows keyword retrieval misses queries that matter.
- **To measure on a permitted sample:** how many redirect targets and templates are missing after a typical selection, how much relevant content sits in unexpanded templates, how large a useful corpus is, and how stale current-revision exports become.

## Sample retrieval evaluation

Nothing was run: no retrieval engine and no permitted real corpus exist. The [#16](https://github.com/whcas/OSRS-MCP/issues/16) dataset (PR #27) and the scorer tracked in #37 separate corpus coverage from ranking quality, so a judged section missing from the corpus is reported as a coverage gap rather than a ranking failure. The real-Wiki benchmark runs only as an opt-in against a permitted real corpus; synthetic fixtures test mechanics offline with their own ground truth. The acquisition options above are not benchmarked.

## Cost and trade-off summary

- **Human effort replaces operator permission as the main cost.** Manual export needs someone to pick pages and download files. The demand-driven list keeps that to the pages that matter, but coverage will be incomplete until enough is imported.
- **Coverage is the main quality risk.** Missing redirect targets, templates and linked pages, plus unexpanded dynamic tables, can hide relevant content. The design makes this visible instead of hiding it.
- **Staleness risk:** exports are current-revision snapshots, and game updates change values. Revision ids, timestamps and import dates let the advisor flag old text rather than trust it.
- **Automated route (parked)** would give freshness at the price of operator load and the consent question; a dump (if one exists) would be cheaper for everyone. Neither blocks the MVP.
- **Storage:** unmeasured beyond the 8-page, 170,106-byte sample. Likely small enough for a local database, but this is an assumption.

## Open questions

1. **Consent (parked):** does Weird Gloop consent to a small identified API client, and under what limits? Is a dump available? Not pursued while automated acquisition is parked, and the operator is not contacted by this project.
2. **Private local index:** is keeping a local index of manually exported pages for personal use permitted, and is that use "non-commercial"? Manual export does not answer this. It is a question for the operator or legal advice; it is not concluded here.
3. **Open-source publication:** does publishing the application code (without Wiki content) as open source affect the non-commercial or ShareAlike terms, or what is permitted for the tool's users?
4. **Excerpts sent to hosted LLMs:** may bounded excerpts be sent to hosted model providers chosen by the user, and is that redistribution, adaptation or neither under BY-NC-SA 3.0?
5. **ShareAlike scope:** is a derived index or a bounded section extract an adaptation that must itself be CC BY-NC-SA?
6. **OSRS Wiki licence row:** the Weird Gloop table has no entry for this wiki. Confirm the licence and any additional terms from the live footer.
7. **Attribution form:** exact wording the operator wants alongside returned sections.
8. **robots.txt and AI crawlers:** confirm that a user-run local client with an honest User-Agent is acceptable, given named AI bots are disallowed.
9. **Manual export and the Terms of Use:** the automated-use clause is not engaged by the application when a human performs the export, but whether the human step is itself covered, and what the export form's own terms say, has not been checked.
10. **Unverified technical points:** API availability and limits, installed extensions (the `/w/Bucket:` path in `robots.txt` and a Bucket link on Weird Gloop Meta suggest the Bucket structured-data extension is in use, unconfirmed), `maxlag` support, and the export schema beyond the maintainer's one sample (other export options, namespaces and history).

## Verification and revisit criteria

- Before indexing or using a real corpus, verify the applicable terms (reuse, attribution, ShareAlike, private indexing, excerpts sent to hosted models) and record the outcome in an ADR. Manual export is not blanket consent.
- Revisit this recommendation if the export schema differs from the sample, if coverage proves too incomplete to be useful, or if the maintainer decides to pursue automated access or contact the operator.
- Re-check the licence, Terms of Use, `robots.txt` and the Real-time Prices page at that time, since they change; quotes here are dated 2026-11-26.
