# ADR-0003: Dependencies for the offline Wiki importer

- Status: Accepted (maintainer decision of 2026-11-26 in the Wiki implementation plan; adopted here only for `quick-xml`)
- Date: 2026-11-27
- Related issues or PRs: #32 (importer), #33 to #37 (later Wiki work), ADR-0002 (acquisition decision, PR #22)

## Context

The Wiki corpus comes from manually downloaded `Special:Export` XML (ADR-0002). Issue #32 parses that XML into typed pages with bounded memory, no network access, no DTD or entity processing and no execution of Wiki content. The export is untrusted input, so the parser must be a maintained library with explicit control over how much it buffers, not a hand-written scanner.

## Decision

- **Parse with [`quick-xml`](https://crates.io/crates/quick-xml) 0.42**, default features only (the crate has none by default). It is a pull parser over `BufRead`, so the importer decides what is buffered and can stop reading at its limits. It does not resolve external entities or fetch external resources, and it reports `<!DOCTYPE>` as an event, which the importer rejects. Its namespace-aware reader checks the export namespace. Optional features (`encoding`, `serialize`, `async-tokio`) stay off: exports are UTF-8, nothing is deserialised through serde, and the importer is synchronous.
- **No hashing crate yet.** The export's per-revision `sha1` element is stored as the opaque string the wiki reported, validated for shape only. It is not recomputed from the text, so no requirement yet needs `sha1` or `sha2`. The first change that must compute or verify a hash (for example change detection in #34) states that requirement and adds the crate then.
- **No date and time crate.** Export timestamps have one fixed shape, whole-second `YYYY-MM-DDTHH:MM:SSZ`. A small strict parser converts them to the shared `UnixSeconds` and keeps the original text. #31 introduces `jiff`; replacing the parser with it later is a local change.
- **Approved for later PRs, not adopted here:** `rusqlite` (bundled SQLite with FTS5) for #34, and `tempfile` as a dev-dependency. Each is added by the first PR that uses it, with its own justification and `scripts/check.sh audit`.

## Alternatives considered

- **`roxmltree`:** builds a whole DOM in memory, so the size of the input bounds memory use only indirectly, and a multi-hundred-megabyte export cannot be streamed. Simpler code, weaker bounds.
- **`xml-rs`:** a streaming parser, but slower and less actively maintained than `quick-xml`.
- **A hand-written scanner:** rejected. XML escaping, character references, CDATA, comments and namespaces are established behaviour that a maintained crate handles better.
- **`sha1` or `sha2` now:** rejected until a concrete requirement exists, as the maintainer decided.

## Consequences

- One new direct dependency. `cargo deny` runs on the PR that adds it.
- `quick-xml` reads one event at a time into a buffer, so the importer wraps its input in a byte-counting reader to cap how much a single page may occupy before any event is complete. That wrapper is part of the importer, not of the dependency.
- The importer does not verify revision hashes. A corrupted text that keeps its reported hash is not detected here.

## Verification and revisit criteria

- Unit tests in the importer cover DOCTYPE and entity rejection, unsupported schema, and the input, page, text and depth limits, all with synthetic XML.
- Revisit if `quick-xml` is flagged as unmaintained, if a later change needs hash verification, or if `jiff` makes the timestamp parser redundant.
