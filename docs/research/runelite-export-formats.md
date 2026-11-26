# RuneLite account export formats

Research for [#15](https://github.com/whcas/OSRS-MCP/issues/15). Status: **research only, nothing verified against an authentic export.** No parser, fixtures or schema are implemented from this document.

## Summary

- RuneLite has **no official or standard account export format**. Account export is provided by independent community plugins on the Plugin Hub, each with its own file layout. "RuneLite export" is therefore not one schema; the first decision is which plugin to support.
- The evidence below comes from public plugin README and listing pages only. Source code, schema documents and real output files were **not** read (see "Evidence and its limits"). Field names given here are as the pages describe them and must not be treated as a verified schema.
- No page shows a format version field. Only one plugin (Gielinor Profile Sync) describes itself as a "versioned account snapshot", and its schema documents were not readable.
- Because no schema is verified, this issue produces **no fixtures** and issue #19 is blocked until the maintainer supplies sanitised samples or chooses a plugin whose schema can be verified.

## Evidence and its limits

Pages were retrieved on 2026-11-26 through a web-fetch helper that summarised them; I did not read the raw pages myself, so quoted details are second-hand. Limits encountered:

- GitHub `src/` trees of `cdfisher/ca-export`, `Lazyfaith/runelite-bank-memory-plugin` and `ReinhardtR/runeprofile-plugin` are disallowed by `robots.txt`, so source files were not read and no workaround was attempted.
- `cwewright/gielinor-profile-sync` links `docs/schema-v1.md` and `docs/capture-schema-v1.md`; fetching returned a rate-limit error, so they remain unread. They are the most promising documented schema found.
- Repository pages showed no last-commit dates, so maintenance status is unknown for GitHub-only plugins.
- Plugin pages are third-party content and were treated as data. Licences stated on repo pages are BSD-2-Clause for the plugins that show one; licence of the export *files* is the user's own data.

## Candidate plugins

Capabilities are as stated on the README or listing page. "Schema shown" means whether the page itself shows field-level detail.

| Plugin (source) | Local file output | Described contents | Schema shown |
| --- | --- | --- | --- |
| OSRS Data Sync (`slappingpikachu/osrs-data-sync`) | `.runelite/plugin-data/osrsdatasync/<player>/` with `progress.json`, `killcounts.json`, `clog.json`, `containers.json`, `loot.jsonl` | Skills, quests, diaries, combat achievements, collection log, bank, inventory, equipment, GE offers, loot | Yes, field-level (see below) |
| Claude Export (`t2traw88/claude-export`) | `.runelite/plugin-data/claude-export/state.json` (plus cache files) | Skills, quests, diaries, equipment, inventory, bank, slayer, boss KC | Key table only |
| Gielinor Profile Sync (`cwewright/gielinor-profile-sync`) | `.runelite/gielinor-profile-sync/latest.json` plus a per-account file | Skills (including Sailing), quests, diaries, bank, inventory, equipment, GE offers | Linked `docs/schema-v1.md`, unread |
| Character Export / OSRS Intelligence (`briantellis/Character-Export`, fork of `DZWNK/Character-Export`) | `.runelite/plugin-data/osrs-intelligence/<character>/` with `character.json`, `quests.json`, `diaries.json`, `combat_achievements.json`, `account.json`, `collection_log.json`, `bank.json`, ... | Also states **account type**; optional upload to a user-chosen server (off by default) | File list only |
| Local Data Exporter (`oph0109/local-data-exporter`, fork of `GoblinTek/local-data-exporter`) | `.runelite/local-data-exporter/latest.json`, `<RSN>.json` | Skills, containers, GE, quests, diaries, account metadata | No |
| Data Exporter (rayports, Plugin Hub listing) | JSON or CSV | Skills, quests, bank, friends, ignores, clan, diaries, combat achievements | No |
| Account Data Exporter (akimpton, Plugin Hub listing) | Local JSON | Account data, bank, inventory, stats, snapshots | No |
| Combat Achievement Exporter (`cdfisher/ca-export`) | `.runelite/ca_exporter/<username>.json` via `::caexport` | Combat achievements only | No |
| Bank Memory (`Lazyfaith/runelite-bank-memory-plugin`) | TSV export from a context menu (since v1.2.0) | Bank items only | No columns shown |

Not file exporters: Wise Old Man and RuneProfile upload to their services; osrsbox-plugins dump game metadata, not account state.

## Details reported for OSRS Data Sync (unverified)

Reported by its README; treat as claims to verify against a real file.

- Each `.json` begins with an `updated` time, described as ISO 8601 UTC. Writes go to a temp file then rename.
- `progress.json`: `player`, `questPoints`, `totalLevel`, `totalXp`, and `skills` as `{"Attack": [level, xp], ...}` holding real (unboosted) levels; `quests` as name to `0|1|2`; `diaries`; `diaryTasks`; `ca`.
- `containers.json`: `bank`, `inventory`, `equipment` as arrays of `[item id, quantity, slot]`; `bankAt` as epoch milliseconds. Noted items are stored as the unnoted id.
- Requires other RuneLite plugins for some data; the collection log is complete only after a one-time in-game search.

## Details reported for Claude Export (unverified)

- `state.json` keys: `player` (name, combat level, world), `skills` (real level, boosted level, XP), `totalLevel`, `questPoints`, `quests` (`NOT_STARTED`/`IN_PROGRESS`/`FINISHED`), `equipment`, `inventory`, `bank` (items with GE price, `totalGeValue`, `lastSeen`), `diaries`, `slayer`, `bossKc`.
- Disabled sections are written empty or null rather than removed, so absent and disabled must be distinguishable from empty.
- Exports on a debounce (at most every five seconds) after changes. Cache files reportedly include RuneLite's internal account id, which is personal data.

## Format ambiguities and risks

1. **No common schema.** The same fact is shaped differently per plugin: skills as `[level, xp]` arrays versus objects with real and boosted levels; quest state as integers versus strings.
2. **Real versus boosted levels.** Some exports report both. Advice must use real levels; the contract must say which field it reads.
3. **Account mode.** Hardcore status (and whether the account has died, becoming a regular ironman) is essential for this project but only Character Export is reported to include account type. Other exports do not state it, so account mode may have to be user-configured rather than imported.
4. **Identity.** Player name is mutable and a display name; stable identifiers are not documented. RuneLite's internal account id appears in cache files and should not be stored.
5. **Timestamps.** Formats differ (ISO 8601 UTC, epoch milliseconds) and some exports are only written on change, so file age is not data age. Per AGENTS.md, preserve source timestamps and distinguish missing or stale values.
6. **Staleness and partial data.** Bank and collection log data are only as fresh as the last time the player opened them in game.
7. **Skill set changes.** Gielinor Profile Sync's page mentions Sailing, a skill newer than most plugin documentation, so skills must not be a closed enum without an unknown/unsupported path.
8. **Versioning.** No version field was documented; schema drift between plugin releases cannot currently be detected.
9. **Third-party plugins.** Maintenance, trust and Plugin Hub status vary; some plugins offer network upload. Importing files read locally avoids that, but the export files themselves are untrusted input and must be validated.

## Proposed minimal character import contract

For discussion; not final until a plugin and real samples are chosen. It deliberately covers only the minimum named in #15 and #19.

**Input.** A single JSON document supplied as bytes or a reader (no hardcoded path).

**Required**
- A skills collection, each entry with skill identifier, real level and XP.

**Optional, preserved only if verified for the chosen plugin**
- Player display name, treated as an unverified label rather than a key.
- Export time (`updated`/equivalent), kept with its original text and parsed value.
- Account type, when the plugin provides it.

**Validation**
- Level is an integer in `1..=99` (virtual levels, if present, are a separate optional field); XP is an integer in `0..=200_000_000`.
- Missing required fields, wrong types and duplicate skills are typed errors, not silent defaults.
- Level and XP must be consistent with the standard XP table; inconsistency is a warning or error to be decided.
- Skill identifiers are parsed into a known set with an explicit unknown variant, so new skills do not fail the whole import.

**Unknown fields.** Recommend ignoring unrecognised top-level and per-skill fields (forward compatibility) while rejecting an unsupported declared format version if one exists. This is a decision for the maintainer.

**Domain model.** Input DTOs per supported plugin are separate from the transport-independent domain `Account`/`Skills` types, so adding another plugin means adding a DTO, not changing the domain.

## Gaps and what is needed from the maintainer

I need the maintainer to decide or provide:

1. **Which plugin to support first.** Recommendation: OSRS Data Sync or Gielinor Profile Sync, since both document structure and produce local files; the maintainer may already use another.
2. **One sanitised real export** from that plugin (player name, account ids and any internal ids replaced), for a hardcore account where possible, from the plugin version in use, including plugin name and version. At minimum the file containing skills (`progress.json` for OSRS Data Sync, `latest.json` for Gielinor Profile Sync).
3. **One sample showing account type** (hardcore, dead hardcore, ironman) if the plugin emits it, to settle where account mode comes from.
4. Optionally, samples for quests, diaries, collection log, combat achievements, bank, inventory and equipment, for later issues.
5. **Decision on unknown-field policy and the level/XP consistency rule** above.
6. Whether to read `docs/schema-v1.md` and the plugin source files manually, since automated retrieval was blocked or rate-limited; pasting them would settle most open schema questions.

## Fixtures

None added. The requirement to prepare fixtures "only where schema is verified" is not met for any plugin. Synthetic fixtures will be added in #19 once a schema is verified, and will be labelled synthetic and kept separate from any authentic samples.
