# RuneLite account export formats

Research for [#15](https://github.com/whcas/OSRS-MCP/issues/15). Status: **source-backed and checked structurally against one authentic export; no parser is implemented here.** The parser is #19.

## Summary

- RuneLite has no official account export format; exports come from independent plugins. The maintainer chose [DZWNK/Character-Export](https://github.com/DZWNK/Character-Export) ("Character State Exporter") as the **first supported plugin**.
- The schema below is read from the plugin source at upstream commit `d790a93` (`CharacterStateExporterPlugin.java`; upstream `main` reports plugin version `0.6.0`). The source was read from a local read-only clone, so unlike the earlier survey nothing here is second-hand.
- Structure and types were also compared with one authentic export on the maintainer's machine (plugin version `0.6.0`), inspected read-only. **Only keys, types and shapes were recorded; no raw values, account names or paths appear in this repository.**
- The minimal import contract (`character.json`: skills, real level, experience, `exported_at`) is concrete and proposed at the end.
- Account mode (Hardcore Main, Hardcore Ironman, dead) must **not** be inferred from the export. `world_types` describes the world the client is logged into, not an account's mode; see "Account mode".

## Evidence and its limits

| Evidence | Status |
| --- | --- |
| `CharacterStateExporterPlugin.java` at `d790a93`: `exportCharacterSnapshot`, `buildStats`, `basePayload`, `writeJson`, `stableJson`, `sanitizeFileName`, `ContainerKind` | Read from a local clone. Authoritative for the shape for plugin 0.6.0 |
| Upstream tests (`CharacterExportRegressionTest`) | Read; they cover diary and combat-achievement helpers only, not `character.json`, so they say nothing about the character schema |
| One authentic export directory (plugin 0.6.0), read-only | Checked for structure, types and internal consistency only. Counts and booleans were derived locally; no values copied |
| Other plugin versions | **Not seen.** Behaviour of versions other than 0.6.0 is unverified |
| Plugin documentation / a published schema | None found; the source is the schema |

The earlier survey of other community plugins was based on README pages read through a summarising fetch tool and never verified against source. It is reduced to a short list at the end and is not a basis for any parser.

## Where the files are

The plugin writes under the RuneLite directory, in a per-account folder:

```text
<RuneLite dir>/character-exporter/<account folder>/<file>.json
```

- `<RuneLite dir>` is `RuneLite.RUNELITE_DIR`, normally `~/.runelite`. The importer needs a configurable location, never a hardcoded one.
- `<account folder>` is the in-game name with every character outside `[a-zA-Z0-9_\- ]` replaced by `_` (`sanitizeFileName`). It is a mutable display-name derivative and personal data: it must not be committed, put on GitHub or used as a key.
- All files are UTF-8 JSON, pretty-printed, with a trailing newline.

| File | Written when | Contents (shape only) |
| --- | --- | --- |
| `character.json` | Logged in and stats ready; on stat changes and manual export | Metadata plus `stats` |
| `quests.json` | Quest varbit changes | `summary` and `quests[]` of `{id, name, state}` |
| `diaries.json` | Diary varbit changes | `summary` and `diaries.<region>.<tier>` of `{complete, tasks_done}` (some tiers also have `tasks`) |
| `combat_achievements.json` | Varbit changes | `summary` and `tiers.<tier>` with `tasks[]` of `{id, name, complete}` |
| `collection_log.json` | Only for log pages opened in game; accumulates across sessions | `tabs.<tab>.<page>` with `items[]` of `{id, name, obtained}` and `last_scraped` |
| `bank.json`, `inventory.json`, `equipment.json`, `seed_vault.json` | Container change; bank and seed vault only after being opened | `kind`, `item_count`, `items[]` of `{slot, id, quantity, name}` |
| `status.json`, `recent_events.json`, `exporter.log` | Only with the plugin's debug logging on | Observability, not account data |

Only `character.json` is in scope for #19.

## `character.json`

Built by `exportCharacterSnapshot` from `basePayload`, account fields and `buildStats`.

| Field | JSON type | Source | Notes |
| --- | --- | --- | --- |
| `exported_at` | string | `OffsetDateTime.now().toString()` | ISO-8601 with offset. See "Timestamps and freshness" |
| `plugin_version` | string | constant | `"0.6.0"` at `d790a93`; the only version marker |
| `session_id` | string | per-session identifier | Not account data; **must not be retained** (maintainer decision) |
| `reason` | string | trigger | For example `stat_changed`, `initial_game_tick`, `manual_command`, `manual_panel`. Informational |
| `account_name` | string or `null` | `player.getName()` | Display name; personal data; mutable. The source can write `null` |
| `world` | integer | `client.getWorld()` | World number |
| `game_state` | string | `GameState.name()` | `LOGGED_IN` whenever a snapshot is written (the method returns early otherwise) |
| `world_types` | array of strings | `WorldType` names | For example `MEMBERS`. Describes the world, not the account |
| `stats` | object | `buildStats` | Skill name to `{real_level, boosted_level, experience}`, all integers |

`stats` keys are RuneLite's `Skill.getName()` values (capitalised, for example `Attack`, `Hitpoints`, `Runecraft`), in enum order, **excluding `OVERALL`** (`isTrackedSkill`). Consequences:

- There is no overall level or total experience field. Any total is derived by the importer and is not authoritative.
- The skill set is whatever the installed RuneLite version defines. The authentic export had 24 skills including `Sailing`, newer than much third-party documentation. Skills must be parsed into a known set **with an explicit unknown path**, never a closed enum that fails the import.
- `real_level` is the unboosted level. `boosted_level` includes temporary boosts and drains and must not be used for advice.

`isCharacterStateReady` requires a non-blank player name and at least one skill with `real_level > 0`, so a written file never has every skill at zero.

## Timestamps and freshness

- `exported_at` is `OffsetDateTime.now().toString()` in the **system default zone**. Java's `toString` omits zero seconds and zero fractions and prints the fraction in groups of three digits, so valid values include `2026-01-01T12:00Z`, `2026-01-01T12:00:00.123Z`, `2026-01-01T12:00:00.123456Z` and `2026-01-01T12:00:00+01:00`. The authentic export used `YYYY-MM-DDTHH:MM:SS.ffffffZ` (UTC, microseconds). The parser must accept all ISO-8601 offset forms, not just that one.
- **`exported_at` is the time of the last change, not the last observation.** `writeJson` compares a `stableJson` that **excludes `exported_at` and `reason`**, and does not rewrite the file if nothing else changed. A file with an old `exported_at` can still describe the current state. Report `exported_at` verbatim, treat age as "no change since", and let the caller decide what is stale. The file modification time has the same limitation.

## Write semantics and consumption

- Writes use `Files.write` with `CREATE`, `TRUNCATE_EXISTING` and `WRITE`: **not atomic** (no temporary file and rename). A reader can see an empty or truncated file while RuneLite writes. The importer must report invalid JSON as a transient, retryable condition distinct from an unsupported schema, and any later file watcher must debounce.
- Character exports are throttled to one per second (`CHARACTER_EXPORT_INTERVAL_MS`).
- Local files are consumed directly from the plugin's directory, with a configurable path, and no manual upload.

## Versioning

There is no schema version field, only `plugin_version`. Drift cannot be detected beyond comparing that string. Proposed policy: accept any `plugin_version`, record it as text, and rely on structural validation.

## Account mode

`world_types` is the set of `WorldType` values of the world the client is on. The authentic export contained `MEMBERS` and a hardcore-main entry. Per the maintainer's decision, **account mode is not inferred from this**: a Hardcore Ironman that dies becomes a regular ironman, and the field belongs to the world. Account mode is a configured input to this project, and `world_types` is kept as observed metadata only. The authentic export is for a Hardcore Main account (not a Hardcore Ironman).

## Verified against an authentic export

Compared read-only, structure and types only (plugin 0.6.0):

- Files present: `character.json`, `quests.json`, `diaries.json`, `combat_achievements.json`, `collection_log.json`, `bank.json`, `inventory.json`, `equipment.json`. Absent: `status.json`, `recent_events.json`, `seed_vault.json`, `exporter.log` (debug logging off; seed vault never opened). A missing file is therefore normal.
- `character.json` top-level keys were exactly the nine in the table above, with the listed types.
- `stats` had 24 skills, each `{real_level, boosted_level, experience}` as integers; no `Overall`; `Sailing` present.
- `exported_at` matched `YYYY-MM-DDTHH:MM:SS.ffffffZ`; `plugin_version` was `0.6.0`.
- Every `real_level` was in `1..=99`, experience in `0..=200_000_000`, `boosted_level >= 0`, and **`real_level` equalled the level implied by the standard experience table (capped at 99) for all 24 skills, `Sailing` included.** No skill exceeded 99 by experience. This supports the consistency rule below, but one export is a small sample.
- The other files matched the source: quest states `FINISHED`, `IN_PROGRESS`, `NOT_STARTED`; combat-achievement tiers `easy` to `grandmaster`; 12 diary regions with `easy`, `medium`, `hard` and `elite`; items as `{slot, id, quantity, name}`.
- `bank.json` and `collection_log.json` had older modification times than `character.json`, as the write rules predict.

Not verified: other plugin versions, a dead or non-hardcore account, unusual account names (sanitised to `_`), and behaviour at 200 million experience.

## Proposed minimal character import contract

Aligned with the maintainer's clarifications on #15 and #19.

**Input.** One `character.json` document as bytes or a reader. No hardcoded path.

**Required**
- `stats`: an object of skills, each with integer `real_level` and `experience`.
- `exported_at`: an ISO-8601 offset date-time, kept as original text alongside the parsed instant.

**Optional or ignored**
- `boosted_level` is parsed if present but not used for advice.
- `plugin_version` is recorded as text; any value is accepted.
- `account_name`, `world`, `game_state`, `world_types` and `reason` are not required. `account_name` is a personal identifier and is **not retained**.
- `session_id` is **not retained**.

**Validation**
- `real_level` is an integer in `1..=99`; `experience` is an integer in `0..=200_000_000`. Anything else is a typed error.
- A level inconsistent with the standard experience table is a **warning, not an error**, applied only where the rule is reliable. Do not assume a cap of 99 beyond what the table defines.
- Missing or non-object `stats`, wrong types and duplicate skills are typed errors. Invalid JSON is reported separately from schema errors because the non-atomic write can produce it transiently.
- Skill names are parsed into a known set with an explicit unknown variant, so new skills do not fail the import. Whether unknown skills are kept or only reported is for #19 to decide and document.

**Unknown fields.** Ignored at every level. There is no declared format version to reject.

**Domain model.** Input DTOs for this plugin are separate from the transport-independent domain type, so adding another plugin means adding a DTO.

## Fixtures

`tests/fixtures/runelite_export/character.synthetic.json` is a **synthetic** document following the shape above. It holds no real account data; its README states the provenance. No authentic export is committed or will be.

## Remaining gaps

1. Plugin versions other than `0.6.0`, and exports from other account states, are unverified.
2. Parsing of `quests.json`, `diaries.json` and the container files is out of scope here and for #19.
3. Whether the domain model keeps `plugin_version` and `exported_at` beyond the importer is a maintainer choice for later issues.

## Other plugins considered, not first priority

OSRS Data Sync, Gielinor Profile Sync, Claude Export, Local Data Exporter and a fork of this plugin (OSRS Intelligence) exist. They were summarised from README pages only, none was read at source, and none is supported by this work. A fork's schema must not be assumed to match this one.
