# OSRS Hardcore Worlds Advisor MCP

A model-agnostic, read-only advisory service for Old School RuneScape Hardcore worlds, built in Rust and exposed via the Model Context Protocol (MCP).

## Purpose

Provide LLM clients with reliable, efficient access to game knowledge, live Hardcore Grand Exchange data and player-specific information. Move factual retrieval and deterministic calculations into software while leaving strategic reasoning and recommendations to the model and player.

## Intended capabilities

- Import and query RuneLite character, quest, inventory and bank exports.
- Retrieve Hardcore-world prices, volumes and timestamps; identify stale or missing data.
- Discover shop arbitrage, trading and processing opportunities, accounting for liquidity, fees and capital requirements.
- Query structured quest requirements, rewards, unlocks, shops and recipes.
- Retrieve relevant OSRS Wiki sections from a local index, retaining source and revision provenance.
- Support account-specific progression, objectives, constraints and history.

## Principles

- Advisory only: no gameplay automation, client input or automated trading.
- Prioritise accurate data and Hardcore survival; account-specific restrictions are configurable.
- Never silently substitute standard-world market data for Hardcore-world data.
- Keep domain logic independent of MCP clients and LLM providers.
- Keep personal data local and out of version control.
- Start simple; measure retrieval quality before adding embeddings or more complex infrastructure.

## Development status

Early development. A minimal read-only MCP server (`osrs-mcp serve`, stdio transport) exposes a single `server_info` tool; no OSRS functionality is implemented yet. Initial milestones are Rust tooling and CI, a minimal MCP server, verification in Claude and ChatGPT, and investigation of Wiki acquisition and licensing. SQLite and local section-based retrieval are proposed directions, not settled implementation requirements.

## Contributing

Read [AGENTS.md](AGENTS.md) for engineering conventions. Use a scoped [GitHub issue](https://github.com/whcas/OSRS-MCP/issues), a short-lived branch and a pull request into `main`. Human review and passing applicable checks are expected before merging. Record significant architectural decisions under [docs/adr](docs/adr/).

## Running the MCP server

Build the binary (the pinned toolchain is installed by rustup automatically):

```sh
cargo build --release --locked
```

The server speaks MCP over stdio: an MCP client launches the process and exchanges newline-delimited JSON-RPC on stdin and stdout. Only protocol messages are written to stdout; diagnostics go to stderr.

```sh
target/release/osrs-mcp serve     # serve MCP on stdio until stdin closes
target/release/osrs-mcp --version # print the version
```

Run it from source without a release build with `cargo run --locked -- serve`. (Cargo's own build output goes to stderr, so stdout stays clean.)

### Quick manual check without an MCP client

This pipes an initialise handshake and a `server_info` call into the server and prints the responses:

```sh
{ printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"server_info","arguments":{}}}'
  sleep 1; } | target/release/osrs-mcp serve
```

The `sleep` keeps stdin open long enough for the server to answer before it sees end of input. The second response contains `"structuredContent":{"capabilities":["tools"],"name":"osrs-mcp","read_only":true,"version":"<version>"}`.

### Tools

| Tool | Description |
| --- | --- |
| `server_info` | Read-only. Returns the server name, version, read-only guarantee and MCP capabilities. Uses no external data. |

## Development

### Toolchain policy

The Rust toolchain is pinned to an exact stable release in [`rust-toolchain.toml`](rust-toolchain.toml) (currently 1.100.0), and `rust-version` in `Cargo.toml` matches it. rustup installs the pinned version automatically, so local and CI results agree. Bump the pin deliberately in its own pull request, usually to the latest stable every six-week release or when a dependency needs it. Dependabot does not update this file.

### Checks

Run all local checks with:

```sh
scripts/check.sh
```

This runs, in order, `cargo fmt --all --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo build --locked --all-targets` and `cargo test --locked`. Each step can also run alone with `scripts/check.sh fmt`, `clippy` or `test`. CI calls the same script, so local and CI commands cannot drift apart.

The dependency audit runs separately because it needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) and network access to the RustSec advisory database. Install the same version CI uses (set in `.github/workflows/ci.yml`):

```sh
cargo install --locked --version 0.20.2 cargo-deny
scripts/check.sh audit
```

It checks advisories, banned crates and crate sources, configured in [`deny.toml`](deny.toml). Licence checks are not enabled yet because the project licence has not been decided.

### Git hooks (optional)

Native Git hooks give quick local feedback before CI. They are opt-in, repository-local and require no framework. The hooks live in [`.githooks/`](.githooks/); enable them in your clone with:

```sh
scripts/install-hooks.sh            # install
scripts/install-hooks.sh uninstall  # remove
```

Installing sets `core.hooksPath=.githooks` in this repository's local Git config only; global and system config are never touched, and running it again is harmless. `.githooks/` and `./.githooks` count as the same directory; an absolute path to it does not. If `core.hooksPath` is already set to something else (in any config scope), the script refuses and explains; pass `--replace` to override it in this repository. Hooks already in `.git/hooks` stop running while `.githooks` is active, and the installer lists any it finds.

Prerequisites: a POSIX shell, Git and the pinned Rust toolchain (see "Toolchain policy"). Unix-like systems only; on Windows use WSL or Git Bash.

| Hook | Runs | Notes |
| --- | --- | --- |
| `pre-commit` | `git diff --cached --check`, then `scripts/check.sh fmt` if any `.rs` file is staged | Fast and non-mutating: it never formats or re-stages files. |
| `pre-push` | `scripts/check.sh` (fmt, Clippy, build, tests) | Excludes `audit`, which needs the network. Skipped when only deleting remote refs. |

Hooks check your working tree, not a snapshot of what will be committed or pushed:

- The whitespace check in `pre-commit` reads the staged changes only. The formatting check reads working-tree files, so with partial staging (`git add -p`) it can pass or fail on content that is not in the commit.
- `pre-push` always runs the full `scripts/check.sh` (build, Clippy and all tests), even for a docs-only push, so the first run after a change can take minutes; `--no-verify` skips it. Pushing a tag also runs the checks and prints the "not the checked-out commit" warning.
- `pre-push` tests the working tree. If it has uncommitted changes, untracked files, or the pushed ref is not the checked-out commit, the hook prints a warning because the result may not describe what is pushed.
- Neither hook replaces CI, which checks the pushed commits and remains authoritative, as do branch protections.

Bypass a hook once with `git commit --no-verify` or `git push --no-verify`. The hooks are tested in throwaway repositories by `tests/git_hooks.rs`, which runs under `cargo test`.

### Lints

`Cargo.toml` sets a strict baseline, and CI denies all warnings:

- **Clippy groups:** `all`, `pedantic` and `nursery` are enabled.
- **Restriction lints:** the restriction group is not enabled wholesale. A selected set targets panics in production code (`unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable` and related lints), direct printing (`print_stdout`, `print_stderr`, `dbg_macro`), keeping stdout free for protocol output, and suppressions that don't say why (`allow_attributes`, `allow_attributes_without_reason`).
- **rustc lints:** unsafe code is forbidden. Several extra lints are on, including `missing_docs`, `missing_debug_implementations`, `unreachable_pub` and `unused_qualifications`.

`clippy.toml` allows panics, unwraps and printing in tests only. When a lint must be silenced elsewhere, use a narrowly scoped `#[expect(lint, reason = "...")]` rather than `#[allow]`. Cargo metadata lints wait until the package metadata and licence are settled (#8).

### CI

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on pull requests, pushes to `main`, a weekly schedule and manual dispatch. It uses read-only repository permissions and needs no secrets. Scheduled runs execute only the dependency audit.

| Check name | Runs | Required on `main` |
| --- | --- | --- |
| `Format` | `scripts/check.sh fmt` | Yes |
| `Clippy` | `scripts/check.sh clippy` | Yes |
| `Test` | `scripts/check.sh test` | Yes |
| `Dependency audit` | `scripts/check.sh audit` | Not yet; a new advisory can fail it on an unrelated change |

The maintainer configures required checks in the branch ruleset; contributors and agents do not change repository protections.

## Licensing and data

Application licensing is not yet established. Third-party game data and Wiki content have separate terms; investigate and document those before acquiring or redistributing substantial datasets. This is an unofficial project, not affiliated with Jagex or the OSRS Wiki.
