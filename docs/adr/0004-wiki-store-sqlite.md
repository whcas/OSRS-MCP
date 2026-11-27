# ADR-0004: SQLite for the local Wiki store

- Status: Accepted (`rusqlite` and `tempfile` approved by the maintainer in the Wiki implementation plan, 2026-11-26)
- Date: 2026-11-27
- Related issues or PRs: #34 (storage and keyword retrieval), ADR-0002, ADR-0003

## Context

Issue #34 persists manually imported pages and their chunks so that later changes can search them offline. The store must survive restarts, make an import all-or-nothing, and later support keyword search with deterministic ranking (FTS5). It is a single-user local tool, so there is no server to run.

## Decision

- **Store pages in `SQLite` through [`rusqlite`](https://crates.io/crates/rusqlite) 0.43** with default features off and only `bundled`. `bundled` compiles `SQLite`, FTS5 included, into the binary, so behaviour does not depend on the system library. Synchronous access matches the importer and the single writer; no async wrapper is added.
- **Schema version in `PRAGMA user_version`**, with one hand-written script per version applied in a single transaction. A file written by a newer version is refused. No migration crate is needed for a handful of tables.
- **One transaction per import**, opened with an immediate write lock. Pages are added one at a time so an export is streamed, and a session that is dropped, or whose caller stops after an error, stores nothing.
- **Only the current revision of a page is kept**, keyed by source and `MediaWiki` page id, with its chunks. A newer revision id replaces it in the same transaction; the same revision with the same content is a no-op; an older or conflicting revision is a typed error. Pages missing from a later export are never deleted.
- **Source identity is the export header's site name, database name and base URL.** It is unverified provenance, so two files that name themselves differently are different sources.
- **The database path is always the caller's choice.** The library has no default location, and nothing is written inside the repository.
- **Search uses a contentless FTS5 table (schema version 2)** over title, section path and chunk wikitext, so the text is not stored a third time. Its row ids are the chunk's, which needed `chunk` rebuilt with an `INTEGER PRIMARY KEY` (the version 1 composite key gives no stable row id across `VACUUM`). Triggers keep the index in step with `chunk`, so no write path can forget it. User words are quoted as literal terms, never passed as FTS syntax; results are ordered by `bm25` with equal column weights, then page id, section index and chunk index, within fixed query and result bounds.
- **`tempfile` (dev only)** supplies temporary database files for tests, so tests exercise the same file-backed code path as production.

## Alternatives considered

- **System `SQLite` (no `bundled`):** a smaller build, but FTS5 availability and the version would vary between machines, which makes tests and results less reproducible.
- **Keeping a revision history table:** nothing in #34 needs old revisions, and they would multiply storage. Can be added with a migration if a use appears.
- **JSON columns for chunk lists:** unnecessary. Section titles, template names and link targets cannot contain a line break, so they are stored newline-separated, which needs no extra dependency or error path.
- **`sled` or flat files:** no ad-hoc querying, and no FTS5 for the later search PR.

## Consequences

- One new direct dependency, which builds a C library (about two minutes on a cold cache), plus `libsqlite3-sys` and small helpers. `cargo deny` passes; no new duplicate-version warnings were added.
- The index is built from raw wikitext, so template and parameter names are searchable and ranked like prose; a hit reports its chunk kind so callers can tell. Column weights are unmeasured defaults to be tuned against the evaluation dataset.
- Page text is stored twice, as the raw revision and again inside its chunks, so that retrieval never has to re-slice. Size is bounded by the import limits.
- A partly stale export is all-or-nothing by default. A caller that wants to keep the fresh pages must import them again without the stale ones.

## Verification and revisit criteria

- Unit tests with temporary database files cover creation, newer-schema refusal, add, replace, unchanged, stale, conflict, rollback on drop and on failure, restart persistence, source separation and error text that contains no page content.
- Revisit if bundled builds become a problem on the maintainer's platforms, if revision history is needed, or if import sizes make the double storage of text matter.
