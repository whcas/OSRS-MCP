# Additional Character Exporter schemas

Research for [#62](https://github.com/whcas/OSRS-MCP/issues/62). Status: **source-backed; no parser is implemented here and no authentic export was read for this document.** It extends [`runelite-export-formats.md`](runelite-export-formats.md) (#15), which stays authoritative for `character.json`, file locations and the shared write semantics.

## Summary

- Every file the plugin writes shares one envelope (`exported_at`, `plugin_version`, `session_id`, `reason`) and one write path, so cross-file rules (freshness, non-atomic writes, absent versus empty) are stated once below.
- **Bank, inventory and equipment share one schema** (`kind`, `item_count`, `items[]` of `{slot, id, quantity, name}`) and can be parsed by one small parser. Quest, diary, combat-achievement and collection-log files each have their own shape.
- The most consequential findings are about **staleness, not syntax**:
  - Throttled exports are **dropped, not deferred**. A change that lands inside the throttle window is not written until the next change.
  - `bank.json` is only written after the bank has been opened, and files are never deleted, so a bank file can be sessions old.
  - Item IDs are **canonicalised**: noted items and placeholders collapse to the base item ID, so a stack of noted items is indistinguishable from the unnoted stack.
  - Diary `tasks` lists exist only if the diary journal was opened in the same session, so their presence is not reliable, and the diary fields are independent observations that need not agree.
  - Combat-achievement per-task data comes from a hard-coded list of 637 struct IDs and silently reports zeros if that list cannot be resolved.
- Recommended parser order: containers (bank, equipment, inventory), then quests, then diaries. Combat achievements and the collection log are identified for later work only.
- Account mode is never inferred from any of these files, including `world_types`.

## Evidence

| Evidence | Pin | Status |
| --- | --- | --- |
| [DZWNK/Character-Export](https://github.com/DZWNK/Character-Export) `CharacterStateExporterPlugin.java` | upstream `main` at `d790a93e24c9d946949f47645b0447210b166d93` (plugin `0.6.0`, same revision as #15; confirmed still HEAD on 2026-11-27) | Read in full for this document. Authoritative for the shapes below at this revision |
| RuneLite API: `ItemManager.canonicalize`, `EquipmentInventorySlot`, `gameval.InventoryID`, `Quest`, `QuestState` | [runelite/runelite `9af7b54deaac35b3153af06195d66c1d68fbea5a`](https://github.com/runelite/runelite/tree/9af7b54deaac35b3153af06195d66c1d68fbea5a) (master on 2026-11-27) | Read to interpret exporter calls. The exporter is built against a pinned RuneLite client version (`build.gradle`), which may differ from this master; treat enum contents as indicative |
| Upstream tests | same revision | Cover diary and combat-achievement helpers only; they say nothing about file shapes |
| Authentic exports | Not accessed | #15 already recorded that bank, inventory, equipment, quest, diary, combat-achievement and collection-log files matched the source structurally. No further local data was read. The checks in "Required local verification" are for the maintainer to run, reporting counts only |
| Other plugin versions | none seen | Unverified |

All examples below are **original synthetic** documents. IDs, names and values are invented and no real account, item stack or progress is represented.

## Cross-file rules

### Envelope

Every file is built from `basePayload`: `exported_at` (string, `OffsetDateTime.now().toString()`, see #15 for accepted forms), `plugin_version` (string), `session_id` (not retained), `reason` (string, informational). Files hold no account name except `character.json`, so **the only account identity on disk is the folder name**, which is personal data and must not be logged or returned.

### Per-file timestamps and freshness

- Each file has its own `exported_at`, and files are written independently (different triggers and throttles). Two files in one directory are **not a consistent snapshot**: an item moved between bank and inventory can appear in both, in neither, or in only one, depending on which file was written last.
- As in #15, `exported_at` is the time of the **last change** of that file's stable payload (`writeJson` skips an unchanged payload), so an old value does not mean stale data and a recent value does not mean current data.
- Throttling (`acquireExportSlot`) **drops** an export that arrives inside the window. Intervals at this revision: character 1 s; bank, equipment and seed vault 1 s; inventory 5 s; quests, diaries and combat achievements 5 s; collection log 2 s. Nothing reschedules the dropped export, so the file keeps the earlier state until the next triggering event. For the inventory this is a realistic way to be one change behind.
- Quest, diary and combat-achievement exports run from `VarbitChanged`, and `onVarbitChanged` schedules nothing while every enabled one of those files was exported within its 5 s window, so a state change that follows an export closely can also be missed until a later varbit change.
- Files are never deleted. After logout, and across sessions, the last written state remains. The envelope cannot tell "written this session" from "written weeks ago" except through `exported_at`.

### Absent, empty and incomplete

| Situation | What is on disk | How it must be reported |
| --- | --- | --- |
| Dataset disabled in plugin settings | No file, or an old one | Not observed |
| `bank.json` / `seed_vault.json`: container never opened (`client.getItemContainer` is `null`, so status `waiting`) | No file, or an old one from an earlier session | **Unknown**, not "empty bank" |
| Container exists and holds nothing | File with `"item_count": 0, "items": []` | Observed empty. Valid and plausible for equipment and inventory |
| Collection log page never scraped | Page absent from `tabs` | Unknown |
| Diary journal never opened this session | `tasks` absent on non-Karamja tiers | Named tasks unknown; `complete` and `tasks_done` remain valid |
| Combat-achievement struct cache failed | `named_data_available: false`, empty `tasks` arrays, `tasks_completed: 0` | **Unknown**, not zero. The code comment says it falls back to varbit counts; the code does not |
| File truncated by a write in progress | Input ends inside a JSON value or is empty | Retryable truncation, per #15 |
| `item_count` differs from `items.length` | Cannot be produced by the exporter | Corrupt or edited: warn, trust `items` |

A parser result must therefore carry the file's `exported_at` and the file kind, and callers must be able to distinguish *not available* from *available and empty*.

### Privacy

- Item holdings, quest and diary progress and the collection log are account-specific data and sensitive in aggregate, even without a name. They must not be committed, logged, placed in error messages or sent to a hosted model without explicit maintainer approval of the data exposure (see #61).
- `name` fields come from the game client and the widget text; treat them as untrusted strings (bounded length, no use in paths or log lines).
- Error types for these parsers should report structural facts only (file kind, field name, count), never values.

## Containers: `bank.json`, `inventory.json`, `equipment.json` (and `seed_vault.json`)

Built by `exportContainerSnapshot`. Same shape for all four; `kind` is the dataset key (`bank`, `inventory`, `equipment`, `seed_vault`).

| Field | JSON type | Source | Notes |
| --- | --- | --- | --- |
| envelope | | | As above |
| `kind` | string | `ContainerKind.getDatasetKey()` | Redundant with the file name; a parser can check they agree |
| `item_count` | integer | `items.size()` | Number of **non-empty rows**, not total quantity |
| `items` | array | container contents | One row per non-empty slot, in ascending slot order |
| `items[].slot` | integer | index into the container array | Raw index. Gaps are normal because empty slots are skipped |
| `items[].id` | integer | `itemManager.canonicalize(id)` | Canonical item ID, see below |
| `items[].quantity` | integer | `Item.getQuantity()` | Java `int`: `1..=2_147_483_647` in a written file |
| `items[].name` | string | `ItemComposition.getName()` | `""` when the lookup fails; a display string, not a key |

Synthetic example (`equipment.json`):

```json
{
  "exported_at": "2026-01-01T12:00:00.123456Z",
  "plugin_version": "0.6.0",
  "session_id": "synthetic-session",
  "reason": "item_container_changed",
  "kind": "equipment",
  "item_count": 3,
  "items": [
    { "slot": 0, "id": 1001, "quantity": 1, "name": "Example helm" },
    { "slot": 3, "id": 1002, "quantity": 1, "name": "Example sword" },
    { "slot": 13, "id": 1003, "quantity": 250, "name": "Example arrow" }
  ]
}
```

### Verified from source

- Rows with `id <= 0` or `quantity <= 0` are skipped. Consequently **placeholders (quantity 0), empty slots and zero stacks never appear**, and a written row always has `id > 0` and `quantity > 0`.
- `id` is `ItemManager.canonicalize`: for a noted item it returns the linked unnoted ID, for a placeholder the real item ID, and otherwise a `WORN_ITEMS` mapping or the same ID. Two consequences: a noted stack looks like an unnoted stack, and the ID is directly comparable to Grand Exchange and item-mapping IDs, which is what this project needs for pricing.
- Equipment `slot` is the `EquipmentInventorySlot` index: head 0, cape 1, amulet 2, weapon 3, body 4, shield 5, arms 6, legs 7, hair 8, gloves 9, boots 10, jaw 11, ring 12, ammo 13. Inventory slots are 0–27 in the standard client. Bank slots are raw container indices; bank tab boundaries are **not** in the file.
- Container IDs used: inventory 93, worn 94, bank 95, seed vault 626 (`gameval.InventoryID`).
- The bank, seed vault and others are exported on `ItemContainerChanged` for a container the client currently holds; bank and seed vault therefore need to be opened in game before the first file exists.
- `item_count` is derived from `items` in the same call, so a mismatch indicates corruption or a different writer.

### Not guaranteed

- **Duplicate item IDs** are possible (multiple non-stackable copies in different slots, or distinct source IDs that canonicalise to one). Slots are unique by construction. A parser must not treat duplicate IDs as an error and must not silently drop rows; aggregation is a consumer decision.
- Whether `slot` order is always ascending is true of the loop but not worth enforcing; do not rely on it.
- Maximum container sizes are a client property, not in the file. A parser should bound row count with a limit well above any real container and reject beyond it, rather than assume a size.
- The inventory row for a stackable item such as a coin stack is one row with a large `quantity`; there is no separate currency field.

### Proposed minimal model and policy

Transport-independent, separate from the input DTO (same approach as `character.json`):

```text
ContainerKind        Bank | Inventory | Equipment      (seed vault: not modelled)
ItemStack            { item_id: ItemId, quantity: Quantity }
ContainerRow         { slot: u16, stack: ItemStack, display_name: Option<BoundedText> }
ContainerSnapshot    { kind, exported_at (text + instant), plugin_version (text), rows: Vec<ContainerRow> }
```

- `name` is retained as an **optional, bounded display name and untrusted data**. Quest and untradeable items need not exist in the Grand Exchange item mapping, so the file may be the only local source of a name, and resolving names by a network call is unnecessary. The `id` is the key; a name is never an identifier, is never used to match or deduplicate rows, and is never trusted as instructions. An empty name (the exporter's failed-lookup value) becomes `None`. Over-long names are rejected or truncated with an explicit marker, a policy for the parser PR to fix (not silently shortened and presented as the original).
- `ItemId` and `Quantity` reuse the shared `crate::domain` types (`ItemId(u32)`, `Quantity` over `NonZeroU64`). The exporter's quantity is a Java `int`, so a quantity above `i32::MAX` is a validation error even though `Quantity` could hold it.
- Typed errors (no values): wrong top-level shape, `items` missing or not an array, row field missing or wrong type, `id` or `quantity` not positive, `quantity` above `i32::MAX`, duplicate `slot`, `kind` present but disagreeing with the requested container, row count above the limit, truncated input, other invalid JSON.
- Warnings (non-fatal): `item_count` mismatch, unknown envelope fields ignored silently, a `name` that is empty (becomes `None`).
- Equipment: map `slot` to a named slot only when it is in `0..=13`; keep an out-of-range slot as a warning rather than failing, since a newer client may add slots. A named equipment-slot type is a separate small decision and should not be required for the container parser.
- Compatibility: accept any `plugin_version`; no schema version exists.
- Duplicate IDs: keep rows as read; aggregation helper (total quantity per item) is deterministic and belongs in the domain layer, not the parser.

### Advisory usefulness

These are useful as standalone, read-only MCP data. They do not need to wait for structured Wiki data: the model can reason over holdings, gear and active-quest state directly, and Hardcore GE prices (#58) can be joined by item ID where a price exists.

- **Bank**: highest value. "What do I already own for method X", "what is my holdings worth at Hardcore GE prices", "what should I sell or keep". Report `exported_at` and that it reflects the last bank open.
- **Equipment**: high value for survival-first advice (is the account under-geared for a target, what upgrade is realistic). Small, and changes less often than the inventory.
- **Inventory**: valuable during active questing (what is carried, what is missing for the next step), with the caveat that it is the most volatile file and the one most affected by dropped throttled writes. Expose it with its own `exported_at`, and with the same freshness wording as the other files.
- **No combined snapshot.** The three files are written independently, so any tool presenting more than one must give each its own timestamp and must not claim an atomic or consistent snapshot (see "Per-file timestamps and freshness").
- **Seed vault**: not useful yet; excluded.

Tool priority, when exposure is designed: bank, equipment and quests first, then inventory, each as its own read-only tool or clearly separated section.

## `quests.json`

Built by `exportQuestSnapshot` from `Quest.values()`.

| Field | JSON type | Notes |
| --- | --- | --- |
| envelope | | As above |
| `summary` | object | `finished`, `in_progress`, `not_started`: integers derived from the same loop |
| `quests` | array | One row per RuneLite `Quest` enum entry (about 220 at the pinned RuneLite commit; includes miniquests and Recipe for Disaster sub-quests) |
| `quests[].id` | integer | **RuneLite's internal quest ID**, not a Wiki or game-cache ID |
| `quests[].name` | string | RuneLite's display name, which can differ from Wiki titles |
| `quests[].state` | string | `FINISHED`, `IN_PROGRESS`, `NOT_STARTED` |

```json
{
  "exported_at": "2026-01-01T12:00Z",
  "plugin_version": "0.6.0",
  "session_id": "synthetic-session",
  "reason": "varbit_changed",
  "summary": { "finished": 1, "in_progress": 1, "not_started": 1 },
  "quests": [
    { "id": 1, "name": "Example Quest A", "state": "FINISHED" },
    { "id": 2, "name": "Example Quest B", "state": "IN_PROGRESS" },
    { "id": 3, "name": "Example Quest C", "state": "NOT_STARTED" }
  ]
}
```

### Verified from source

- `Quest.getState` runs a client script and maps the result `2 → FINISHED`, `1 → NOT_STARTED`, **anything else → `IN_PROGRESS`**. An unexpected script result therefore shows as `IN_PROGRESS`.
- The exporter counts anything that is neither `FINISHED` nor `IN_PROGRESS` as not started.
- The list is whatever the RuneLite build knows. A quest added to the game but not yet to that RuneLite build is **absent**, which means unknown, not "not started".
- The first snapshot is taken on login (`game_state_changed`). Whether quest varbits are populated at that moment is not established from source; see the uncertainty list.

### Proposed minimal model and policy

```text
QuestState           Finished | InProgress | NotStarted
QuestProgress        { exported_at, plugin_version, quests: Vec<{ runelite_id: u32, name: String, state: QuestState }> }
```

- Keep `runelite_id` and `name` **verbatim**. Mapping to structured quest data (requirements, rewards) is the consumer's job and must be explicit; do not match by name in the parser.
- Unknown `state` string is a typed error (do not guess), since a renamed state could flip advice. This is the one place where failing is safer than an unknown variant.
- Duplicate `id` is a typed error. `summary` is ignored for the model; a mismatch with the rows is a warning.
- A quest missing from the list is *not recorded*; the model must not offer a default `NotStarted` for it.
- Warning for a plausible-but-suspect snapshot: zero `FINISHED` and zero `IN_PROGRESS` across the whole list, which on an established account indicates a snapshot taken before varps loaded.

### Advisory usefulness

High and cheap: which quests are finished or in progress is core progression advice and the list is small. Quest progress is useful as a standalone read-only MCP tool without waiting for structured requirement data; the model can combine it with Wiki knowledge. Requirement-aware advice ("which quests can I start now") later needs the structured quest dataset and an explicit RuneLite-ID-to-Wiki mapping, which are separate work. Prioritise it alongside the bank and equipment tools.

## `diaries.json`

Built by `exportDiarySnapshot`.

| Field | JSON type | Notes |
| --- | --- | --- |
| `summary.tiers_complete`, `summary.tiers_possible` | integer | Computed over the 12 areas × 4 tiers; `tiers_possible` is 48 |
| `summary.note` | string | Static text |
| `diaries.<area>.<tier>.complete` | boolean | Completion varbit `== 1` |
| `diaries.<area>.<tier>.tasks_done` | integer | Count varbit; **no total is in the file** |
| `diaries.<area>.<tier>.tasks` | array, optional | `{name, complete}`; see below |

Areas (fixed order): Ardougne, Desert, Falador, Fremennik, Kandarin, Karamja, `Kourend & Kebos`, `Lumbridge & Draynor`, Morytania, Varrock, `Western Provinces`, Wilderness. Tiers: `easy`, `medium`, `hard`, `elite`.

```json
{
  "exported_at": "2026-01-01T12:00:00Z",
  "plugin_version": "0.6.0",
  "session_id": "synthetic-session",
  "reason": "varbit_changed",
  "summary": { "tiers_complete": 1, "tiers_possible": 48, "note": "synthetic" },
  "diaries": {
    "Varrock": {
      "easy": { "complete": true, "tasks_done": 11 },
      "medium": { "complete": false, "tasks_done": 3 }
    }
  }
}
```

(Real files list all 12 areas and four tiers each; the example is truncated for brevity and a parser should still tolerate absent areas.)

### Verified from source

- `complete` and `tasks_done` come from varbits and are always written. They are **observations** of two separate varbits per tier. This document does not claim they agree with each other or with the in-game journal: the reviewer of this PR reports inconsistencies between these fields in authentic samples that this author has not seen (see "Uncertainties").
- `tasks` is present for Karamja easy, medium and hard (hard-coded names and per-task varbits, `complete > 0`) and elsewhere **only** when `diaryWidgetCache` holds scraped journal text. That cache is filled when the journal widget loads and is **cleared on logout and not restored from disk**, unlike the collection log. A later write in a session where the journal was not opened therefore **removes** `tasks`. Named-task data is opportunistic, free text from the game, and should not be parsed.
- Karamja elite has no per-task list.
- Because totals are absent, "x of y done" requires a per-tier task total from structured Wiki data. `complete == true` should imply `tasks_done` equals that total.

### Proposed minimal model and policy

```text
DiaryTier        Easy | Medium | Hard | Elite
DiaryProgress    { exported_at, plugin_version, areas: Map<AreaName(String), Map<DiaryTier, { complete: bool, tasks_done: u16, tasks: Option<Vec<{ name: BoundedText, complete: bool }>> }>>, warnings }
```

- Area names kept as strings (new regions would otherwise break the import); unknown tier key → warning and skipped, not an error.
- Treat `complete`, `tasks_done` and `tasks` as independent observations, not verified authoritative progression. The parser **warns rather than silently reconciling** when they disagree (for example `complete` is true while a listed task is incomplete, or `tasks_done` differs from the number of completed tasks in a present `tasks` list). It never overrides one field with another.
- Keep `tasks`, when present, as optional data: `{name, complete}` with a bounded, untrusted `name`. The names are **not parsed into requirements** or matched to the Wiki. Their presence is unreliable (see above), so absence means "not observed this session", never "none".
- With known per-tier task totals from structured data, a warning for `complete` true with a smaller `tasks_done`, or `tasks_done` above the total, is a later consumer concern, not the parser's.
- Absent area or tier is unknown, not incomplete.

### Advisory usefulness

Medium: diaries gate real rewards and are a common progression goal. Because the fields can disagree, any exposure must label them as observations and surface the parser's warnings. Land after quests; the "x of y" presentation needs per-tier task totals from structured data.

## `combat_achievements.json` (identified for later)

Built by `exportCombatAchievementSnapshot`.

- Top level: `summary` (`total_tasks_completed`, `total_tiers_completed`, `named_data_available`) and `tiers.<tier>` for `easy`, `medium`, `hard`, `elite`, `master`, `grandmaster`, each `{complete, tasks_completed, tasks_total, tasks[]}` with `tasks[]` of `{id, name, complete}`.
- Task completion is a bit in one of 20 packed varps (`id / 32` selects the varp, `id % 32` the bit); tier `complete` is a separate varbit.
- Task names and IDs come from a **hard-coded list of 637 struct IDs** (source comment: derived from the osrs-reldo task store). Tasks added by newer game updates are missing. A struct that fails to load is skipped; an out-of-range tier is **silently reclassified as easy** (`tier = 1`); `tasks_total` counts only the tasks the list resolved.
- When the cache is empty, `named_data_available` is `false` and counts are zero; zero then means unknown.
- Hardcore relevance is real (several tiers gate gear and bosses), but it needs combat-achievement reward and task data first. Defer until that dataset exists. Parser risks to record: reliance on `named_data_available`, treat `tasks_total` as informational, treat unknown tier keys as warnings.

## `collection_log.json` (identified for later)

Built by `scrapeCollectionLogPage` and `writeCollectionLogSnapshot`.

- Top level: `entries_scraped`, `note`, `tabs.<tab>.<entry>` for tabs `Bosses`, `Raids`, `Clues`, `Minigames`, `Other`, each entry `{tab, obtained_count, total_items, items[], last_scraped}` with `items[]` of `{id, name, obtained}` (canonicalised IDs).
- It is built by **screen-scraping the interface** and is accumulated across sessions (restored from disk on login). `obtained` is inferred from widget opacity (`0` means obtained); the interface title and items child IDs are hard-coded and the exporter logs when it cannot find them. A client update can break it silently.
- Entries are keyed by entry name only in the cache, so identically named entries in different tabs would overwrite each other; unknown tab indexes fall back to `Other`.
- Pages never opened are absent, so a missing page is unknown. Each entry has its own `last_scraped` (separate from the file `exported_at`).
- Least reliable of the datasets and the least useful for progression advice. Defer; if ever parsed, treat as best-effort and per-entry time-stamped.

## Recommended delivery boundaries

All after #31 merges (parsers should reuse its conventions) and independent of #60 and #61, because parsers stay pure and file I/O lives in #60.

| # | Proposed ticket | Scope | Depends on |
| --- | --- | --- | --- |
| A | Parse `bank.json`, `inventory.json`, `equipment.json` into `ContainerSnapshot` | One pure parser, shared DTO, typed errors and warnings, original synthetic fixtures for each kind, absent/empty cases, duplicate-ID and slot-uniqueness tests. No file I/O, no MCP | #31 |
| B | Parse `quests.json` into `QuestProgress` | Pure parser as above; unknown-state and zero-progress behaviour tested | #31 (A not required) |
| C | Parse `diaries.json` into `DiaryProgress` | Pure parser; fields kept as observations with warnings on disagreement; optional `tasks` kept as data, not interpreted; absent versus empty tested | #31 |
| D | Read additional export files through the configured directory | Extends #60's bounded reader with more fixed filenames; no path input from callers | #60, plus A/B/C as wanted |
| E | Read-only MCP tools for bank, equipment and quest status, then inventory | Each with an agreed schema, its own per-file `exported_at` and no combined-snapshot claim. Does not wait for structured Wiki data; the bank can join Hardcore GE prices by item ID where a price exists | D, A/B, and #63 tool registration |
| F (deferred) | Combat achievements and collection log | Only once reward and task datasets exist | — |

A, B and C keep all three container types in one parser. They are independently reviewable and may be parallelised; A is the largest only in fixture count, not in logic. Splitting equipment slot naming from A is an option if review feels heavy.

## Uncertainties

Facts that this document could **not** establish from source alone:

1. Whether quest, diary and combat-achievement varps and varbits are populated when the first login snapshot runs. A snapshot with all quests `NOT_STARTED` right after login might be written and corrected by a later varbit change, or might persist.
2. The exact set of RuneLite builds the plugin targets over time. The RuneLite code read here is current master, not necessarily the client version the plugin was compiled against.
3. Maximum row counts for bank and seed vault in the current client.
4. Whether `ItemContainerChanged` for the bank fires on every relevant change (for example a deposit-all, a bank tab move) and what the resulting container indices look like with tabs.
5. How noted, placeholder or variant items appear in the real bank, beyond what `canonicalize` does by source reading.
6. Behaviour for other plugin versions, other account types and a dead Hardcore account, and whether character-exporter data changes for a dead account.
7. Equipment slots above 13 or other quirks in newer clients.
8. **Diary field consistency.** A reviewer who has seen authentic user-provided samples reports inconsistencies between the diary tier flags, the `tasks_done` counts and the optional named tasks. This author has not seen those samples and has not read any authentic export for this document, so the nature and frequency of the inconsistencies is unknown. It needs a local investigation with the maintainer's permission, reporting counts only (for example how many tiers have `complete` true with `tasks_done` below the tier's known total). The upstream source reading explains one mechanism, the session-only `tasks` cache, but not necessarily the observed differences.
9. Full upstream validation. The pinned source was read for this document, but this PR has not been independently re-verified against every code path, and no authentic check was run.

## Required local verification (opt-in, counts only)

To be run by the maintainer on their own machine, **only if they choose to**. None of this needs any value to leave the machine; it prints booleans and counts and nothing else. Do not paste file contents anywhere. These filters were exercised against synthetic files only.

Containers (run once per file; a path is deliberately a shell variable the maintainer sets locally):

```sh
jq '{
  items_is_array: (.items | type == "array"),
  count_matches:  (.item_count == (.items | length)),
  rows:           (.items | length),
  positive_qty:   ([.items[].quantity > 0] | all),
  positive_id:    ([.items[].id > 0] | all),
  int_fields:     ([.items[] | (.slot, .id, .quantity) | type == "number" and . == floor] | all),
  slots_unique:   (([.items[].slot] | length) == ([.items[].slot] | unique | length)),
  duplicate_ids:  (([.items[].id] | length) - ([.items[].id] | unique | length)),
  qty_fits_i32:   ([.items[].quantity <= 2147483647] | all),
  empty_names:    ([.items[] | select(.name == "")] | length)
}' "$EXPORT_FILE"
```

For `equipment.json`, additionally check `[.items[].slot] | all(. >= 0 and . <= 13)`.

Quests:

```sh
jq '{
  rows:        (.quests | length),
  ids_unique:  (([.quests[].id] | length) == ([.quests[].id] | unique | length)),
  state_kinds: ([.quests[].state] | unique)
}' "$EXPORT_FILE"
```

Record in the PR or issue only the booleans and counts (for example "all positive: true; duplicate IDs: 0; rows: n"), never item or quest content. Whether to compare container state with the game screen (to answer uncertainties 1, 4 and 5) needs the maintainer's own judgement and is not something to send to a model.

## Out of scope and untouched

No parser, file discovery, MCP tool, dependency or fixture is added. `tests/fixtures/runelite_export/` is unchanged. Nothing here is derived from any authentic export or personal path.
