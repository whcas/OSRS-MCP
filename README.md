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

Early development. No server or application features are implemented yet. Initial milestones are Rust tooling and CI, a minimal MCP server, verification in Claude and ChatGPT, and investigation of Wiki acquisition and licensing. SQLite and local section-based retrieval are proposed directions, not settled implementation requirements.

## Contributing

Read [AGENTS.md](AGENTS.md) for engineering conventions. Use a scoped [GitHub issue](https://github.com/whcas/OSRS-MCP/issues), a short-lived branch and a pull request into `main`. Human review and passing applicable checks are expected before merging. Record significant architectural decisions under [docs/adr](docs/adr/).

Build and test commands will be documented once the Rust project and CI are introduced.

## Licensing and data

Application licensing is not yet established. Third-party game data and Wiki content have separate terms; investigate and document those before acquiring or redistributing substantial datasets. This is an unofficial project, not affiliated with Jagex or the OSRS Wiki.
