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

Wiring this variable into the running server and the `character_summary` tool is tracked in #61. Until then the reader exists as a library type, `osrs_mcp::runelite::LocalExportSource`.

## What is read, and limits

- Only the fixed file name `character.json`. Callers cannot pass a path, and no other file in the directory is opened.
- At most 256 KiB (`MAX_CHARACTER_EXPORT_BYTES`; a real file is a few kilobytes). Larger files are rejected without being parsed.
- Only a regular file: a directory, or a named pipe, named `character.json` is refused. Symbolic links are followed, because the directory is your own.
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
