# Local RuneLite exports

How the server reads your RuneLite Character Exporter data. The data stays on your machine, is read on demand and is never committed, cached or transmitted by this module.

Format details and the reasoning behind these rules are in [`research/runelite-export-formats.md`](research/runelite-export-formats.md).

## Configuration

Set one optional environment variable in the MCP client's server configuration:

| Variable | Meaning |
| --- | --- |
| `OSRS_MCP_RUNELITE_EXPORT_DIR` | Absolute path of **one account's** exporter folder, the directory that contains `character.json` |

- The exporter writes `<RuneLite dir>/character-exporter/<account folder>/character.json`. Point the variable at the `<account folder>`. The server does not discover, list or choose accounts, and never infers anything from the folder name.
- Unset or empty means not configured. The server still starts and serves every unrelated tool; only account tools report that nothing is configured.
- An absolute path is recommended: a relative path is resolved against the server's working directory, which MCP clients do not guarantee.
- The path is checked when a read happens, not at startup, so a wrong path never prevents the server from starting.

`osrs-mcp serve` reads the variable at startup and passes it to the `character_summary` tool (below). In an MCP client configuration it goes in the server's `env`, for example `"env": { "OSRS_MCP_RUNELITE_EXPORT_DIR": "<your exporter folder>" }`.

## What is read, and limits

- Only the fixed file name `character.json`. Callers cannot pass a path, and no other file in the directory is opened.
- At most 256 KiB (`MAX_CHARACTER_EXPORT_BYTES`; a real file is a few kilobytes). Larger files are rejected without being parsed.
- Only a regular file: a directory, or a named pipe, named `character.json` is refused. Symbolic links are followed, because the directory is your own.
- **Trusted directory assumption.** The file type is checked just before the file is opened. That guards against a mistaken file, not against someone who can modify the directory concurrently (for example replacing the file with a named pipe between the check and the open). The directory is the plugin's own, user-owned folder and is assumed not to be modified adversarially. Hardened handling would need platform-specific code and is not provided.
- Every call reads the file again. There is no cache, file watcher, polling or background work.
- Parsing is `parse_character_export`; warnings and `exported_at` are preserved. `exported_at` is the time of the last change to the exported data, not the time of the read or a sign that the player is online.

## Errors

Errors are typed (`ExportReadError`) and deliberately contain no path, file content or operating-system message; `Debug` output shows only whether a directory is configured.

| Error | Meaning |
| --- | --- |
| `NotConfigured` | The variable is unset or empty |
| `DirectoryUnavailable` | The path does not exist or is not a directory |
| `FileNotFound` | The directory has no `character.json` (for example the plugin has not exported yet) |
| `NotARegularFile` | `character.json` is not a regular file |
| `PermissionDenied` | The operating system refused access |
| `TooLarge` | Over the size limit |
| `Io` | Another read failure, identified by kind only |
| `Parse` | Content is not an acceptable export; `is_transient()` is true when the file ended unexpectedly |

The plugin does not write atomically, so a read can catch a half-written file. That is reported as a transient `Parse` error and is **not retried** here; the caller decides whether to read again. No stale or invented value is ever returned.

## The `character_summary` tool

Read-only, no input. Reads `character.json` on each call and returns skill names, real levels and experience, with:

- `exported_at` (verbatim and as UTC), `read_at` (the server's clock, UTC) and `seconds_since_last_change`. The age is **elapsed time since the last change to the exported data, not staleness**: the plugin does not rewrite an unchanged file, and the timestamp does not show whether the player is online. If `exported_at` is later than `read_at` the age is `null` and `exported_at_in_future` is true.
- `plugin_version` (null if absent; `plugin_version_suppressed` is true if the file has a value that does not look like a version number).
- `warnings`, for example a level that disagrees with its experience, or a skill name this server does not recognise. Unrecognised skills are still listed, with `recognised: false`.
- Bounds: at most 64 skills and 64 warnings (`skill_count` and `warning_count` give the totals, `*_truncated` says if any were omitted) and names are cut to 32 characters with `name_truncated` set. Nothing is invented to fill a gap.

Never returned: account name, world, world types, session id, boosted levels, paths or raw file content. Account mode (for example Hardcore) is not inferred from anything in the file.

A failure is an error result (`isError: true`) whose text is JSON with a fixed `status`: `not_configured`, `directory_unavailable`, `file_not_found`, `unreadable` (with `reason`: `not_a_regular_file`, `too_large`, `permission_denied` or `io_error`) or `invalid_export`. `invalid_export` adds `retryable` (true when the file ended unexpectedly, which may be a write in progress but is not proof of one) and a `detail` that never quotes the file. There is no stale fallback.

Example prompt once configured: "Show my skill levels and tell me how old the export is."
