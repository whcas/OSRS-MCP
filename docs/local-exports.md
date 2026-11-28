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

- Only the fixed file names `character.json`, `bank.json`, `inventory.json`, `equipment.json`, `quests.json` and `diaries.json`. Callers cannot pass a path, and no other file in the directory is opened.
- At most 256 KiB for `character.json` (`MAX_CHARACTER_EXPORT_BYTES`; a real file is a few kilobytes) at most `MAX_CONTAINER_EXPORT_BYTES` (4 MiB) for each container file and at most `MAX_QUEST_EXPORT_BYTES` (1 MiB) for `quests.json` and at most `MAX_DIARY_EXPORT_BYTES` (2 MiB) for `diaries.json`; the last three are the parsers' own limits. Larger files are rejected without being parsed.
- Only a regular file: a directory, or a named pipe, with a supported file name is refused. Symbolic links are followed, because the directory is your own.
- **Trusted directory assumption.** The file type is checked just before the file is opened. That guards against a mistaken file, not against someone who can modify the directory concurrently (for example replacing the file with a named pipe between the check and the open). The directory is the plugin's own, user-owned folder and is assumed not to be modified adversarially. Hardened handling would need platform-specific code and is not provided.
- Every call reads the file again. There is no cache, file watcher, polling or background work.
- Container files are read with `LocalExportSource::read_container`. A missing container file is `FileNotFound`, never an empty container: the exporter writes a container only after observing it, and an old file can remain from an earlier session. An observed empty container is a valid snapshot with no rows.
- `quests.json` is read with `LocalExportSource::read_quests`. A missing file is `FileNotFound`, never "no quest started", and a quest absent from a valid file is unrecorded rather than not started.
- `diaries.json` is read with `LocalExportSource::read_diaries`. A missing file is `FileNotFound`, never "no diary task done". The tier completion flag, the `tasks_done` count and the optional task list are independent observations that can disagree; the parser reports warnings and the reader reconciles nothing.
- Each file has its own `exported_at`. Reads of different files are separate observations, not one consistent snapshot of the account.
- Parsing is `parse_character_export`; warnings and `exported_at` are preserved. `exported_at` is the time of the last change to the exported data, not the time of the read or a sign that the player is online.

## Errors

Errors are typed (`ExportReadError`) and deliberately contain no path, file content or operating-system message; `Debug` output shows only whether a directory is configured.

| Error | Meaning |
| --- | --- |
| `NotConfigured` | The variable is unset or empty |
| `DirectoryUnavailable` | The path does not exist or is not a directory |
| `FileNotFound` | The directory has no file for the requested dataset (for example the plugin has not exported yet) |
| `NotARegularFile` | The dataset's file is not a regular file |
| `PermissionDenied` | The operating system refused access |
| `TooLarge` | Over the size limit |
| `Io` | Another read failure, identified by kind only |
| `Parse` | Content is not an acceptable export (the source error is a `ParseFailure` for the dataset); `is_transient()` is true when the file ended unexpectedly |

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

## The `bank_contents` tool

Read-only. Reads `bank.json` on each call and returns one page of rows. Input (all optional): `offset` (rows to skip, default 0), `limit` (1 to 200, default 100) and `item_ids` (1 to 50 exact item IDs; an empty or longer list is rejected, and repeated IDs are ignored). There is no path or file input.

- `rows` keep the file's order. Each has `slot`, `item_id`, `quantity` and an optional `name` (untrusted display text cut to 32 characters, with `name_truncated`). **Repeated item IDs are separate rows**: the exporter lists one row per slot and they are not merged. Identify items by `item_id`.
- `total_rows`, `offset`, `limit` and `truncated` describe the page. An offset past the end is an empty page, not a failure. `snapshot_rows` is the number of rows in the whole file.
- **`item_ids` filter.** The whole file is searched for the requested IDs first; `offset` and `limit` then apply to the matching rows, which keep the file's order and include every matching slot (repeated IDs in different slots are all returned). `total_rows` counts the matching rows before paging. `item_filter.requested_item_ids` echoes the request without repeats and `item_filter.unmatched_item_ids` lists requested IDs with no row anywhere in the file, computed before paging, so an ID whose rows are on another page is never reported absent. Without the filter, `item_filter` is null. Names are not searched: resolve names with `item_search`. An empty bank with a filter is `ok` with every ID unmatched; a missing file is still `file_not_found`.
- An observed empty bank is `status: ok` with `total_rows: 0`. A missing `bank.json` is `file_not_found`, never an empty bank: the exporter writes the bank only after it has been opened, files are not deleted, so a file can also be sessions old. `exported_at` is that file's own time of last change.
- The same timing fields as `character_summary` (`exported_at`, `read_at`, `seconds_since_last_change`, `exported_at_in_future`, `plugin_version`). This file is **not** guaranteed consistent with any other export file.
- `warnings` currently report a file whose own `item_count` differs from its rows (`item_count_mismatch`); the rows are returned as listed.
- No prices, valuation or totals; no account name, world, session id, path or raw JSON.

Failures use the same fixed statuses as `character_summary`, with messages naming `bank.json`.

## The `equipment_contents` tool

Read-only, no input. Reads `equipment.json` on each call and returns the same row shape, timing fields, warnings and failure statuses as `bank_contents`, without paging (a character has at most 14 equipment slots; `slot` is the exporter's index, and a row outside the documented 0 to 13 range produces an `unknown_equipment_slot` warning with the row's position).

- An observed empty equipment file is `status: ok` with no rows, which is a valid state. A missing `equipment.json` is `file_not_found`, never "nothing equipped".
- Equipment is a separate observation with its own `exported_at`; it is not guaranteed consistent with the bank or inventory files.
- No prices, valuation or totals; no account name, world, session id, path or raw JSON.

## The `quest_status` tool

Read-only. Reads `quests.json` on each call. Input (all optional): `state` (`finished`, `in_progress` or `not_started`), `offset` (matching quests to skip, default 0) and `limit` (1 to 250, default 100). There is no path or file input.

- `counts` (`recorded`, `finished`, `in_progress`, `not_started`) are computed over **every recorded quest** and ignore `state`, `offset` and `limit`. `matched` is the number of quests after the `state` filter and before paging; the filter is applied before `offset` and `limit`. `truncated` says whether matching quests follow the returned page. An offset past the end is an empty page, not a failure.
- `quests` keep the file's order. Each has `runelite_id`, `name` and `state`. **`runelite_id` is RuneLite's internal quest ID, not a Wiki or game-cache ID**, and `name` is RuneLite's display text (untrusted, cut to 64 characters with `name_truncated`); it can differ from the Wiki title. Nothing here maps quests to Wiki pages, checks requirements or says which quests are available.
- **A quest absent from the file is unrecorded, not "not started"**, and is not counted. A missing `quests.json` is `file_not_found`, never an empty list. A file that records an empty list is `ok` with `counts.recorded: 0`.
- `warnings` are advisory and never change the quests: `summary_mismatch` (the file's own counters disagree with its rows) and `all_not_started` (every recorded quest is not started, which can be a new account or a snapshot taken before the client finished loading).
- The same timing fields as the other account tools (`exported_at`, `read_at`, `seconds_since_last_change`, `exported_at_in_future`, `plugin_version`). The file is **not** guaranteed consistent with any other export file.
- No account name, world, session id, path or raw JSON.

Failures use the same fixed statuses as `character_summary`, with messages naming `quests.json`.
