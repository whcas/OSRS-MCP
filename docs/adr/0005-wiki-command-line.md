# ADR-0005: Command-line interface for the local Wiki

- Status: Proposed (`clap` approved by the maintainer for the first subcommand beyond `serve`)
- Date: 2026-11-28
- Related issues or PRs: #34 (Wiki CLI for import, search and page), ADR-0002, ADR-0004

## Context

The importer, store and keyword search exist as library code only. To build a database, try queries and read pages, a maintainer needs commands that work offline on manually exported files. The binary previously hand-parsed a single `serve` command, which does not scale to subcommands with flags, values and help.

## Decision

- **`clap` (derive, default features off) parses arguments.** Enabled features are `std`, `derive`, `help`, `usage`, `error-context` and `env`; colour, suggestions and wrapping are left out. It replaces the hand-written parser.
- **Commands live under `wiki`:** `osrs-mcp wiki import <EXPORT_XML>`, `wiki search <WORD>...` and `wiki page <TITLE>`, each with `--db <PATH>`. The `wiki` namespace keeps the top level for `serve` and later tool areas.
- **Layers:** `clap` types (`cli`) turn arguments into plain requests; `wiki::commands` runs them against the importer and store and returns typed reports or errors; `cli::render` turns reports into text; only `main` prints and chooses exit codes. No `clap` type reaches the library commands, and the commands are testable without a terminal.
- **The database path is always explicit:** `--db`, or `OSRS_MCP_WIKI_DB`. There is no default location, so nothing is created unless the user chooses a path. Only `import` creates or upgrades a database. `search` and `page` open it read-only (`Store::open_existing`), validate that the schema is the current version with the store's tables and never run migrations: a missing, empty, outdated or newer file fails and is left unchanged.
- **An import is one transaction.** A stream error or a stale or conflicting revision stores nothing from that file. Pages that are individually unusable are counted in the report and skipped, as in the importer.
- **A database that holds several sources** needs `--site <SITE_NAME>` for `search` and `page`; with one source it is implied. Ambiguity is reported, never resolved silently.
- **Output:** results go to stdout and diagnostics to stderr, so stdout stays clean for output (and, for `serve`, for MCP). Usage errors exit 2, command failures exit 1. Control characters in page text are replaced when printed, because Wiki content is untrusted. Errors never include page text, titles or file paths.
- **The CLI module is public in the library** so the binary and tests can share it (a binary-only module collides between rustc's `unreachable_pub` and Clippy's `redundant_pub_crate`). It is not a stable interface.

## Alternatives considered

- **Top-level `import`, `search`, `page`:** shorter, but crowds the namespace that MCP-related commands will also use.
- **A default database location:** convenient, but writes outside the repository without the user choosing where.
- **Keeping the hand-written parser:** no new dependency, but each flag, error message and help text would be bespoke code.
- **`anyhow` in the commands:** rejected for library code; typed errors keep distinctions and avoid leaking input.

## Consequences

- One new direct dependency (`clap` and its small parser crates). `cargo deny` passes.
- `--help` and `--version` now print to stdout and exit 0, as is conventional; before, usage was printed on stderr with exit 2. Invalid usage is unchanged (stderr, exit 2).
- Search is strict AND over literal words (see ADR-0004); a query with a missing or misspelt word returns no matches. A bounded fallback is a separate, evaluated change (#37).

## Verification and revisit criteria

- Unit tests cover grammar and bounds, rendering, and the commands against temporary databases; `tests/wiki_cli.rs` runs the built binary on a synthetic export (import, repeat import, search, page, redirects, stale revision, missing database, environment variable, usage errors).
- Revisit if a default location is wanted, if output needs a machine-readable format, or when commands for other data areas arrive.
