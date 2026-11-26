# Connecting MCP clients

How to run the local `osrs-mcp` server from Claude Code and Claude Desktop, and
what has and has not been verified.

The server is read-only and uses the **stdio** transport: the client launches
`osrs-mcp serve` as a child process and talks JSON-RPC over its stdin and
stdout. Nothing is exposed on the network.

## Prerequisites

Build the binary once (see the README for toolchain details):

```sh
cargo build --release --locked
```

The binary is `target/release/osrs-mcp`. Use its **absolute path** in client
configuration; GUI clients do not start in the repository directory and do not
inherit your shell `PATH`. Rebuild after pulling changes; clients keep using the
old binary until the server process is restarted.

## Claude Code

Register the server for the current project (stored in your local, per-project
Claude Code config; nothing is committed):

```sh
claude mcp add osrs -- /absolute/path/to/OSRS-MCP/target/release/osrs-mcp serve
claude mcp list        # runs a health check: expect "✓ Connected"
```

Or share a project-scoped configuration with a `.mcp.json` at the project root
(this repository does not commit one, because the path is machine-specific):

```json
{
  "mcpServers": {
    "osrs": {
      "command": "/absolute/path/to/OSRS-MCP/target/release/osrs-mcp",
      "args": ["serve"]
    }
  }
}
```

Claude Code asks you to approve project-scoped servers the first time; until
then `claude mcp list` shows "Pending approval". Inside a session, `/mcp` lists
connected servers and their tools. The tool is exposed as `mcp__osrs__server_info`.

Remove with `claude mcp remove osrs`.

## Claude Desktop

> The Claude Desktop steps below follow Anthropic's published configuration
> format but have **not yet been run** against the app (see the verification
> record); menu names and log paths may differ between app versions.

Edit the Claude Desktop configuration file (**Settings → Developer → Edit
Config** opens it):

| OS | File |
| --- | --- |
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` |

```json
{
  "mcpServers": {
    "osrs": {
      "command": "/absolute/path/to/OSRS-MCP/target/release/osrs-mcp",
      "args": ["serve"]
    }
  }
}
```

If the file already has other keys or servers, merge the `osrs` entry into the
existing `mcpServers` object. Fully quit and restart Claude Desktop (closing the
window is not enough on macOS). The server should appear under **Settings →
Developer** as running, and the tool under the chat's tools menu. Ask: "Use the
osrs server_info tool and show me the result." Claude Desktop asks for approval
before the first tool call.

If it does not connect, check the MCP log: `~/Library/Logs/Claude/mcp*.log` on
macOS (`%APPDATA%\Claude\logs` on Windows). The server writes a startup line to
stderr, which clients capture in these logs.

## Expected result

Calling `server_info` returns this structured content (version follows
`Cargo.toml`):

```json
{
  "name": "osrs-mcp",
  "version": "0.1.0",
  "read_only": true,
  "capabilities": ["tools"]
}
```

## Verification record

Automated checks and manual client verification are listed separately. A client
is only marked verified when the result was actually observed.

### Automated (run in CI via `scripts/check.sh`)

These test the protocol, not any specific client:

- `tests/mcp_server.rs`: an rmcp client completes the handshake, discovers the
  read-only `server_info` tool and calls it.
- `tests/stdio_server.rs`: the real binary, driven over stdio with raw JSON-RPC;
  stdout contains protocol messages only and diagnostics go to stderr.

### Manual client verification

| Client | Status | Evidence |
| --- | --- | --- |
| Claude Code | Verified in a Linux cloud container (not on the maintainer's workstation) | See below |
| Claude Desktop | **Not verified** | Blocked: needs the maintainer's desktop app. See "Pending" below. |

#### Claude Code (verified, 2026-11-26)

- Client: Claude Code 2.1.332, Ubuntu 24.04 x86_64 cloud container, headless
  (`claude -p`) mode.
- Server: `osrs-mcp` 0.1.0 built from commit `de855be` of the #3 branch
  (release build), launched over stdio.
- Discovery: `claude mcp add osrs -- <path>/osrs-mcp serve` then
  `claude mcp list` reported `osrs: … - ✓ Connected`.
- Invocation: `claude -p "Call the osrs MCP server's server_info tool and reply
  with exactly the JSON it returned" --allowedTools mcp__osrs__server_info`
  returned `{"capabilities":["tools"],"name":"osrs-mcp","read_only":true,"version":"0.1.0"}`
  after a tool-using turn (3 turns, no error).
- Project-scoped `.mcp.json` (shown above) was also exercised: `claude mcp list`
  reported "Pending approval" until approved, which is expected, and the same
  call via `--mcp-config` returned version `0.1.0`.
- Restrictions: headless mode needs the tool allow-listed (`--allowedTools`);
  interactive sessions prompt for permission instead. The interactive `/mcp`
  screen was not exercised, and this was not run on the maintainer's own
  machine or Claude Code installation.

#### Claude Desktop (pending maintainer action)

Not verified: Claude Desktop is a GUI application on the maintainer's computer
and the server must be built for that machine's OS and architecture. To close
this out, follow "Claude Desktop" above, then record here:

- [ ] Claude Desktop version (Claude menu → About) and OS:
- [ ] `osrs-mcp` version and commit built:
- [ ] Server shows as running under Settings → Developer: yes / no
- [ ] `server_info` listed among the chat's tools: yes / no
- [ ] Tool call result observed (paste or screenshot; mask any private paths):
- [ ] Restrictions encountered (approval prompts, restart needed, log locations):

If the server does not connect, include the relevant lines from the MCP log in
the issue (#4) so the cause can be diagnosed.
