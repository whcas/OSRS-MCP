# ChatGPT integration investigation

Investigation for #5: can the minimal MCP server (#3) be used from the
maintainer's ChatGPT project and subscription?

**Status: documented from OpenAI's official pages on 2026-11-26. Nothing below
has been tested against a real ChatGPT account.** Every scenario is marked
*Documented* (stated by OpenAI) or *Untested* (not exercised by us). The
maintainer's plan and the ChatGPT desktop version are unknown to us, so plan
eligibility is unresolved.

## Sources

- [Add custom MCP server](https://developers.openai.com/api/docs/guides/developer-mode.md) (OpenAI developer docs)
- [Connect and test your plugin](https://developers.openai.com/plugins/deploy/connect-chatgpt) (OpenAI developer docs; formerly under `/apps-sdk/`)
- [Model Context Protocol in ChatGPT and Codex](https://learn.chatgpt.com/docs/extend/mcp.md)
- [Developer settings](https://learn.chatgpt.com/docs/developer-settings) (its MCP section was only partly read; it says the desktop app and Codex CLI/IDE share one `config.toml`)
- [Desktop-only plugins](https://developers.openai.com/pt-BR/plugins/deploy/desktop-only) (read in machine-translated Portuguese)
- The help-centre article on developer mode (help.openai.com, article 12584461) could not be retrieved, so plan eligibility and beta status were not confirmed from it.

These pages carry no revision dates. OpenAI renames and moves this feature
often ("developer mode", "apps", "connectors", "plugins"), so re-check before
relying on the details.

## Two separate things: the Codex host and hosted ChatGPT chats

OpenAI's documentation distinguishes two places a tool can be used, and a
result in one says nothing about the other:

- **Codex host** (ChatGPT desktop app, Codex CLI, IDE extension). They share
  MCP configuration (`~/.codex/config.toml`, or a trusted project's
  `.codex/config.toml`) and can run **local** stdio servers. The "Settings →
  MCP servers" screen in the desktop app edits this Codex configuration.
- **Hosted ChatGPT chats, including ChatGPT web, Work chats and Projects.**
  These use plugin tools backed by **remote** MCP servers. OpenAI states that
  ChatGPT web "doesn't read local Codex configuration files" and that local MCP
  servers and transports may not be available in the cloud.

So configuring `osrs-mcp` for Codex does **not** make it available inside a
ChatGPT Project, and testing in one must not be reported as proof for the
other. Whether the maintainer's ChatGPT Project can reach any custom server is
only documented via the remote plugin path.

## Findings

| Path | What OpenAI documents | Fits our server today? |
| --- | --- | --- |
| ChatGPT web, custom MCP server | Remote servers only, over streamable HTTP or SSE. The endpoint must be reachable at a public HTTPS URL (typically ending `/mcp`) or through a "Secure MCP Tunnel". Authentication: OAuth or none. Both read and write tools are supported; tools without a `readOnlyHint` are treated as writes and confirmed. Workspace policies (including Lockdown) apply. | No: we only have stdio. Needs a streamable HTTP transport and a reachable endpoint. |
| Codex host: ChatGPT desktop app | Settings → MCP servers → Add server, choosing STDIO or Streamable HTTP. This edits the Codex host configuration shared with the CLI and IDE extension. Not documented as visible to hosted chats or Projects. | Plausibly yes for **Codex-local use only**, *untested*. |
| Codex host: CLI / IDE extension | `codex mcp add <name> -- <command>` or a `[mcp_servers.<name>]` table with `command` and `args` in `~/.codex/config.toml`. Default `startup_timeout_sec` 10 and `tool_timeout_sec` 60. | Plausibly yes for Codex-local use only, *untested*. Whether Codex is included in the maintainer's subscription is not stated on the pages read. |
| Hosted ChatGPT chat / Project | Uses plugins with remote MCP tools; does not read local Codex configuration; local servers may be unavailable in the cloud. | No: needs the remote path in the first row, and is unrelated to the Codex-local results. |
| Desktop-only plugins (stdio) | Reserved for OpenAI-approved partners, who must request authorisation with an organisation ID. | No: not self-serve. |

Not established from the pages read: which subscription plans can add custom
MCP servers, whether custom servers work inside ChatGPT **Projects**, and tool
count limits.

## Compatibility of the current server

- Transport: stdio only. This is what the Codex host supports locally; it
  cannot be used from hosted ChatGPT chats or Projects, which need a remote
  server.
- The `server_info` tool is annotated `readOnlyHint: true`, so it would not be
  treated as a write action.
- The server's `instructions` string is short, within the guidance to keep the
  first 512 characters self-contained.
- Startup is well inside Codex's 10 s default `startup_timeout_sec` (the binary
  starts in milliseconds).

## Recommended fallback and next steps

1. **Try the Codex-local stdio path** (no hosting, no network exposure). This
   tests the Codex host only. On the maintainer's machine, build the release
   binary and add it to the ChatGPT desktop app's Codex MCP settings (Settings
   → MCP servers → STDIO, command = absolute path to `osrs-mcp`, argument
   `serve`) or to Codex directly:

   ```toml
   # ~/.codex/config.toml
   [mcp_servers.osrs]
   command = "/absolute/path/to/OSRS-MCP/target/release/osrs-mcp"
   args = ["serve"]
   ```

   Then ask Codex to call `server_info`. Record the app version, plan, OS and
   the observed result, or the error, below.
2. **Test the actual ChatGPT Project separately.** Open the maintainer's
   project and check whether any custom tool or plugin can be added there at
   all. Based on the documentation this requires the remote path below, so
   expect it to fail with the stdio server. A Codex success does not substitute
   for this.
3. **If neither works**, use Claude Code or Claude Desktop for now (#4) and
   treat hosted ChatGPT as requiring remote hosting.
4. **Requirements for future remote hosting** (a separate issue and ADR; not
   started here):
   - A streamable HTTP transport (rmcp provides one behind a feature flag),
     serving the MCP endpoint at `/mcp`.
   - A public HTTPS endpoint, or OpenAI's Secure MCP Tunnel pointed at a local
     server, which avoids a public listener.
   - Authentication appropriate to the data: none is acceptable only for public,
     non-personal tools. Anything that touches player exports or account data
     needs OAuth, and per AGENTS.md remote exposure needs explicit maintainer
     approval.
   - Keep tools read-only and annotated so ChatGPT does not prompt for
     confirmation on reads.
   - Re-check how workspace policies and the maintainer's plan affect custom
     servers.

Per the issue, no server was exposed publicly and no credentials were used or
committed in this investigation.

## Manual verification record (to be filled in by the maintainer)

| Scenario | Status | Evidence |
| --- | --- | --- |
| Codex-local: ChatGPT desktop app (Codex host) + stdio | Untested | |
| Codex-local: Codex CLI or IDE extension + stdio | Untested | |
| Actual ChatGPT Project (hosted) can add any custom tool or plugin | Untested | |
| Actual ChatGPT Project + remote server (tunnel or hosted) | Untested, and requires new server work | |

The first two rows prove nothing about the last two, and vice versa.

For each tested scenario record: client and version, plan, OS, transport,
configuration used (mask paths), the result of calling `server_info`, and any
restriction encountered.
