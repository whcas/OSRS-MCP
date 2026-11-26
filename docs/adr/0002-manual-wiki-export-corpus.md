# ADR-0002: Build the Wiki corpus from manually exported XML

- Status: Accepted for the acquisition decision only; the licensing and reuse questions under Consequences remain open
- Date: 2026-11-26
- Related issues or PRs: #6 (decision), #22 (investigation), #16 and #37 (evaluation), #32 to #36 (implementation)

## Context

Retrieval needs OSRS Wiki sections with provenance. The host's Terms of Use prohibit automated use without prior consent, and `robots.txt` disallows `api.php` and `Special:` pages for generic agents. No database dump was found. The maintainer supplied one real `Special:Export` sample that shows page and revision metadata, raw wikitext, explicit redirects whose targets are absent, nested templates, inline tables and unexpanded generated tables. Details and evidence are in [the investigation](../investigations/wiki-acquisition.md).

## Decision

- The MVP corpus comes from **human-selected, manually downloaded `Special:Export` XML files** (current revision by default), imported locally and offline. The application and its agents do not fetch Wiki pages, automate the export form, crawl or contact the operator.
- Store each page revision with its page id, namespace, title, revision id, timestamp and content hash, and split it into sections for retrieval with the provenance needed for attribution.
- Keep raw wikitext. Do not try to render MediaWiki initially. Mark unexpanded templates and tables rather than hiding them.
- Record missing redirect targets, templates and requested pages explicitly, and keep a persistent, deduplicated, paste-ready list of titles for the human to export. Titles enter the list only through an explicit local request action (CLI, or an MCP tool that is off by default, annotated as not read-only and enabled by the user); search, retrieval and import never add new requests. A successful import may transactionally mark existing outstanding requests satisfied (including a refresh request once an adequate revision arrives), but it never enqueues new ones. Request only known, relevant missing pages, not links recursively.
- A section that is only unexpanded template markup in an already imported article is a rendering limitation, not a missing page. It is reported as unsupported content, and exporting the same article again does not resolve it. Only a specific missing template or page that is known to be relevant is requested.
- Evaluation reports corpus coverage separately from ranking quality (see #16 and #37).
- Exports and raw Wiki text stay out of Git. Tests use synthetic fixtures with their own ground truth.

## Alternatives considered

- **MediaWiki API client:** the best technical fit, but needs operator consent. Parked.
- **Official dump:** existence unknown, and would still need the operator. Parked.
- **Scraping or automating the export form:** engages the automated-use clause. Rejected.
- **Community datasets:** structured data only, no article prose. Not a retrieval source.

## Consequences

- No automated-access question blocks the MVP, at the cost of ongoing human effort and incomplete, snapshot coverage.
- Manual export is **not** consent for reuse, attribution, ShareAlike, private indexing or sending excerpts to hosted models. Those questions stay open and real-corpus use must wait until the applicable terms are verified.
- The export schema is known from one sample only.

## Verification and revisit criteria

- Verify the applicable terms before indexing or using a real corpus, and record the outcome in an ADR.
- Revisit if the schema differs from the sample, if coverage is too incomplete to be useful, or if the maintainer decides to pursue automated access.
