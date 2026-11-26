# ADR-0001: Use rmcp with a stdio transport for the MCP server

- Status: Accepted
- Date: 2026-11-26
- Related issues or PRs: #3, #23

## Context

The project needs an MCP server that Claude Code, Claude Desktop and other
clients can launch locally, with the protocol layer kept separate from domain
logic. Issue #3 asks for the official Rust SDK, a local stdio transport,
protocol output isolated from diagnostics, and a first read-only tool.

## Decision

- Depend on [`rmcp`](https://crates.io/crates/rmcp) 3.11, the official Rust MCP
  SDK, with default features disabled and only `server`, `macros` and
  `transport-io` enabled. Remote transports, authentication and the client side
  are not compiled into the binary.
- Use `tokio` (current-thread runtime, built by hand in `main`) because rmcp is
  async-only, and `serde` for tool output types. `schemars` is used through
  rmcp's re-export (`#[schemars(crate = "rmcp::schemars")]`) so its version
  always matches the SDK's.
- Serve over stdio via `osrs-mcp serve`. Stdout carries protocol messages only;
  startup and failure diagnostics are written to stderr. This is covered by a
  process-level test.
- Keep the MCP adapter in `src/mcp.rs`. Tools return typed structured content
  (with an output schema) and carry read-only annotations. The first tool,
  `server_info`, returns package metadata and uses no external data.
- The rmcp client feature is a dev-dependency only, used to drive the server in
  tests.

## Alternatives considered

- **A hand-written JSON-RPC server:** avoids a dependency, but re-implements
  protocol negotiation and schema generation that the official SDK maintains.
- **Streamable HTTP first:** needed eventually for clients that cannot launch
  local processes (see #5), but adds hosting and authentication concerns that
  are out of scope for the first milestone.
- **A different Rust MCP crate:** rejected in favour of the official SDK.

## Consequences

- rmcp 3.x pulls in `chrono`, which RustSec flags as unmaintained
  (RUSTSEC-2026-0543, no safe upgrade). It cannot be disabled through features.
  The maintainer accepted a narrowly scoped exception for exactly this advisory
  (decision recorded in #23): `deny.toml` ignores it with a reason, and no other
  advisory or dependency is covered. #23 stays open to track removal and to
  review the exception on every rmcp upgrade.
- rmcp's `#[tool_handler]` macro generates `async` trait methods without
  `.await`, which trips `clippy::unused_async_trait_impl`. A single narrow
  `#[expect]` with a reason covers the generated impl; the project's lint
  baseline is otherwise unchanged.
- Tools use the `#[tool]` macro; protocol types stay in `src/mcp.rs` so domain
  code does not depend on rmcp.
- The dependency tree grows substantially; builds take longer.

## Verification and revisit criteria

- In-process tests (`tests/mcp_server.rs`) and process-level stdio tests
  (`tests/stdio_server.rs`) cover initialisation, tool discovery, invocation
  and stdout isolation.
- Revisit if rmcp drops `chrono`, if a required client cannot use stdio (add a
  remote transport behind a separate decision), or if rmcp's API churn becomes a
  maintenance burden.
