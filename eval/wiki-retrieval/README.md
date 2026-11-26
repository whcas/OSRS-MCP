# OSRS Wiki retrieval evaluation dataset

A small, versioned benchmark for comparing future keyword and semantic retrieval over OSRS Wiki sections. It measures **retrieval** only: whether the right article sections come back, how much text comes back and how fast. It does not measure whether a model's final advice is correct (see [Limitations](#limitations)).

No retrieval engine, index or Wiki download exists yet, so this directory holds the dataset, its format and the evaluation procedure, and no results.

## Files

| File | Purpose |
| --- | --- |
| `dataset.v1.json` | Queries, relevance judgements and the page records they refer to |
| `README.md` | This document: format, metrics, procedure, limitations |
| `../../tests/eval_dataset.rs` | Offline structural checks, run by `scripts/check.sh` |

Dataset content is original: query wording is written for this project, and the references are page titles, section heading names and revision identifiers. No Wiki prose is copied, which keeps the file clear of share-alike questions (see [issue #6](https://github.com/whcas/OSRS-MCP/issues/6) on Wiki acquisition and licensing). No personal data or account exports are included.

## Format (`format_version` 1)

Top-level fields:

| Field | Meaning |
| --- | --- |
| `format_version` | Integer. Changes only for breaking format changes |
| `dataset_id`, `dataset_version` | Identity of the benchmark, and a semantic version for its content |
| `created` | Creation date (`YYYY-MM-DD`) |
| `source` | Site, observation date, method used to check references, and a licence note |
| `modes` | The game modes a query can concern (see [Modes](#modes)) |
| `grades` | Meaning of relevance grades `1` and `2` |
| `pages` | Page records referred to by judgements |
| `queries` | The benchmark queries |
| `retired_query_ids` | Ids removed from the set, with the reason. A retired id is never reused |

### Page record

| Field | Meaning |
| --- | --- |
| `title` | Canonical page title after redirects, as observed |
| `requested_as` | Redirect source titles that were requested and resolved to this page |
| `oldid`, `last_modified` | Permanent revision id and date, both `null` when not observed |
| `revision_basis` | `footer` if `oldid` and `last_modified` were read from the page footer text, otherwise `not_observed`. Revision ids reported only by a summarising fetch tool are deliberately **not** recorded, because such tools can get them wrong |
| `headings_complete` | `true` if the page's whole heading list was seen; `false` if only part of it was. This describes the page, whereas `references` (below) describes the judged sections only |
| `observed_sections` | Section paths seen on the page |
| `note` | Optional caveat |

A **section path** is heading texts from the top level down, joined by ` > `, for example `Walkthrough > Preparing for the fight`. A section path names a section by heading text, which can change between revisions; the revision fields say which text was observed.

### Query record

| Field | Meaning |
| --- | --- |
| `id` | Stable identifier `qNNN`; never reused |
| `category` | `exact_name`, `multi_entity`, `game_mechanic`, `quest`, `skilling`, `hardcore_safety` or `money_making` |
| `mode` | `hardcore_worlds`, `hardcore_ironman` or `mode_independent` |
| `query` | The text given to the retrieval system |
| `intent` | Why the query is in the set |
| `references` | Whether the judged pages and sections were seen to exist: `verified`, `partial` or `unverified` |
| `relevance` | How far the grades have been checked: `provisional`, `reviewed`, `ambiguous` or `unresolved` |
| `tags` | Free labels such as `redirect`, `lexical_gap` (the query avoids the page's own words), `cross_page`, `numeric_range`, `exploratory` |
| `judgements` | List of `{page, section, grade}`. `section` is a section path, or `null` for the page as a whole |
| `notes` | Required unless `references` is `verified` and `relevance` is `provisional` or `reviewed` |

**Grades:** `2` means the section directly answers the query; `1` means useful supporting context. Anything not listed is treated as not relevant. A query's judgements never overlap: no two judgements on one page may name the same section, a section and its ancestor, or a whole page and any of its sections. This keeps a single result from satisfying two judgements.

**Two separate checks.** Existence and relevance are tracked apart because they have different evidence:

- `references` records whether the page and section **exist**. `verified` means every judged page and section heading was seen in the page's verbatim contents block, even if the page's full heading list was not (`headings_complete: false`); `partial` means some judged headings were only inferred from a tool summary; `unverified` means none could be observed. For pages with `revision_basis: not_observed` (most of them), "verified" says the headings existed on the observation date, not which revision they came from.
- `relevance` records whether the **grades are right**. Every grade in the current dataset is `provisional`: it is the author's judgement from page and section titles, and no section text has been read or reviewed by a person. `reviewed` is reserved for a query whose grades a human has checked against the section text (none yet). `ambiguous` means several answers are defensible or the answer lies in pages not checked. `unresolved` means no reference could be established.

Provisional grades are not verified ground truth. Do not describe a score computed from them as a measure of correctness.

**Scoring tiers:**

| `relevance` | `references` | How to report |
| --- | --- | --- |
| `reviewed` | `verified` | Headline figures (no query qualifies yet) |
| `reviewed` or `provisional` | `verified` or `partial` | Provisional figures, always labelled provisional and reported separately from reviewed ones |
| `ambiguous` or `unresolved` | any | Excluded from scores; list them separately |

Until human review happens, every score from this dataset is provisional.

Ambiguous and unresolved queries are kept so the gaps stay visible, rather than inventing ground truth.

### Modes

The project targets **Hardcore Worlds**, the fresh-economy, one-life mode. **Hardcore Ironman** is a different, older account mode with a similar name, and a retrieval system must not conflate them. Each query carries a `mode`:

- `hardcore_worlds`: concerns the targeted mode.
- `hardcore_ironman`: included only as explicit contrast or disambiguation (`q003`, `q018`). The test suite requires these queries to be tagged `disambiguation` and to stay a minority.
- `mode_independent`: general game knowledge that applies in any mode.

Report results for the `hardcore_worlds` queries separately, as well as overall, because that subset is the closest to the project's goal. It is small, so treat it as a smoke signal.

### Coverage

25 queries: 3 exact-name, 4 multi-entity, 4 game-mechanic, 2 quest, 2 skilling, 7 Hardcore safety and 3 money-making. By mode: 5 Hardcore Worlds, 2 Hardcore Ironman contrast and 18 mode-independent. By checks: 18 `verified`/`provisional`, 5 `partial`/`provisional`, 1 `ambiguous` (`q023`) and 1 `unresolved` (`q025`, the Hardcore Worlds economy, which has no dedicated section on the Hardcore Worlds page). Several queries deliberately test redirects (`fairy rings`), wording that avoids the page's own terms (`lexical_gap`) and answers spread over more than one page. `retired_query_ids` lists four earlier queries that were replaced by Hardcore Worlds queries to stay near 25.

## Matching rules

A retrieval result is `(page, section path)`, with the page title resolved through redirects to the canonical title in the dataset.

- **Section match:** the result matches a judgement if it is on the same page and its section path equals the judged path or is a descendant of it. A judgement whose `section` is `null` matches any section of that page.
- **Whole-page judgements.** A judgement whose `section` is `null` is satisfied by any section of that page, so section recall on a query whose grade-2 judgements are all page-level is effectively page recall. Leave such queries out of section-level aggregates and report them at page level only.
- **Page match:** the result is on the same page as the judgement, regardless of section. Use this for engines that return whole pages.
- A result that is an ancestor of the judged section (a larger chunk containing it) counts as a page match only.
- **No double counting.** Judgements in a query never overlap (checked by the tests), so a result matches at most one judgement. A judgement is satisfied once, however many results match it: several chunks from the same section count as one hit for recall. Duplicate results still occupy ranks, so they lower the rank of everything behind them.

## Metrics

Compute each metric over the scored queries and report it per category and per mode as well as overall; label all figures provisional (see [Scoring tiers](#coverage)). Relevant items for a query are its judgements with grade `2`, unless stated.

| Metric | Definition |
| --- | --- |
| Section recall@k (k = 1, 3, 5, 10) | Grade-2 judgements satisfied within the top k results, divided by the number of grade-2 judgements |
| Page recall@k | **Distinct** grade-2 pages seen within the top k results, divided by the number of **distinct** grade-2 pages. A page counts once however many grade-2 sections it has |
| Hit@k | 1 if any grade-2 judgement is satisfied in the top k, else 0 |
| Reciprocal rank | 1 / rank of the first result matching any grade-2 judgement, 0 if none. The mean over queries is MRR |
| nDCG@10 (optional) | Grades 1 and 2 as gains, section-level matching, each judgement counted once at the rank of its first matching result |
| Response size | Characters (and, when available, model tokens) in the text returned for the top k results; report mean and the largest |
| Latency | Wall-clock time per query. Report the median and the 95th percentile, separately for cold start and warm runs, with the measurement machine described |
| Index metadata | Corpus revision or fetch date, index size and build time, recorded once per run |

A single number across all metrics is deliberately not defined; compare engines on quality and cost together.

### Worked examples

Rankings below are invented to show the counting rules.

**Two grade-2 sections on one page (`q009`).** The grade-2 judgements are `Aggressiveness > Mechanics > Aggression range` and `Aggressiveness > Mechanics > Hunt range`. That is 2 section judgements and 1 distinct page. Suppose the top 3 results are:

1. `Aggressiveness > Mechanics > Aggression range`
2. `Aggressiveness > Mechanics > Aggression range` (a second chunk of the same section)
3. `Aggressiveness > Notes`

- Section recall@3 = 1/2 = 0.5. The repeated chunk does not satisfy a second judgement, and `Notes` matches nothing at section level.
- Page recall@3 = 1/1 = 1.0, because the distinct grade-2 page was found once.
- Reciprocal rank = 1/1 = 1.0. Hit@1 = 1.

**Two grade-2 pages (`q004`).** The grade-2 judgements are `Rune crossbow > Comparison between other ranged weapons` and `Armadyl crossbow > Comparison between other ranged weapons`. Suppose the top 3 results are:

1. `Rune crossbow > Comparison between other ranged weapons`
2. `Rune crossbow > Combat stats` (grade 1, not counted for recall)
3. `Armadyl crossbow > Comparison between other ranged weapons`

- Section recall@1 = 1/2 and recall@2 = 1/2 (rank 2 is only grade 1); recall@3 = 2/2 = 1.0.
- Page recall@3 = 2/2 = 1.0; page recall@1 = 1/2.
- Reciprocal rank = 1.0.

**A result that is too coarse.** If an engine returns the whole page `Aggressiveness` as one result for `q009`, it is an ancestor of the judged sections: page recall counts it (1/1) and section recall does not (0/2).

## Coverage-aware evaluation

The real corpus is expected to grow incrementally from manually exported Wiki XML (see [issue #6](https://github.com/whcas/OSRS-MCP/issues/6) and the acquisition investigation in PR #22), so a judged page or section may simply not have been imported yet. A plain recall score would blame the engine for that. Evaluation therefore separates two questions:

- **Corpus coverage:** was the expected page and section actually imported, is a redirect's target present, and is the expected content usable text rather than only unexpanded templates?
- **Ranking quality over available material:** given that the expected material is in the corpus, does the engine rank it well?

This dataset stays the same for every corpus. Availability is **computed by the scorer against a given corpus** and is never stored in the dataset.

### Availability of a judgement

For each judgement, the scorer assigns one state against the corpus under test:

| State | Meaning |
| --- | --- |
| `available` | The page (after redirect resolution) is in the corpus and, for a section judgement, the section path is present with usable text |
| `redirect_target_missing` | The expected title is a known redirect, but its target page was not imported |
| `page_missing` | Neither the page nor a redirect to it was imported |
| `section_missing` | The page was imported but the section path was not found, for example because of a renamed heading or a different revision |
| `template_only` | The section exists, but its substance sits in unexpanded templates or dynamically generated tables that are not searchable text |
| `unknown` | Availability cannot be determined (for example, the corpus carries no record of what was requested) |

Rules:

1. **Never drop an unavailable judgement silently**, and never count one as a ranking success. It stays in the denominator of every end-to-end figure.
2. **Report three views side by side:**
   - *Coverage:* grade-2 judgements that are `available`, divided by all grade-2 judgements, with a count for each unavailable state.
   - *Conditional* recall@k, hit@k and MRR: computed over `available` grade-2 judgements only. A query with none available is left out of conditional figures and is counted in coverage.
   - *End-to-end* recall@k, hit@k and MRR: computed over all grade-2 judgements, with unavailable ones counted as not satisfied and marked as caused by coverage, not ranking.
3. **Report title resolution separately.** For queries that test names and redirects (category `exact_name`, tag `redirect`), report whether the title resolves to the canonical page, resolves only to a redirect with a missing target, or does not resolve. This is distinct from section results and from page-level deduplication, which follow the matching rules above.
4. **Keep section and page levels apart.** A page can be available while one of its judged sections is not.
5. **Produce a missing-material report:** two separate parts, with no Wiki text.
   - *Missing pages:* the deduplicated titles behind every `redirect_target_missing` and `page_missing` state, plus any specific missing template or page that a `template_only` section depends on and that is known to be relevant. One title per line, paste-ready for a human. It may feed the demand-driven request list in the application, but the scorer only reports: it never fetches anything, never changes the queue, and never queues outgoing links recursively.
   - *Unsupported or unresolved content:* `template_only` sections whose page is already imported are listed separately, by page and section, as a rendering or expansion limitation. Re-exporting an article that is already in the corpus does not fix this, so these entries are not request titles.
6. **Keep provisional labels.** Coverage figures count grade-2 judgements, whose grades are provisional (see above), so every coverage, conditional and end-to-end figure is labelled provisional until the relevant queries are `reviewed`.
7. **Say what the corpus is.** Every run records whether the corpus is `real` or `synthetic`. A synthetic corpus needs its own ground truth and must not reuse these references. Real-corpus scoring is opt-in and subject to the permissions that apply to using Wiki content; nothing here acquires pages automatically or sends excerpts to hosted models.

### Coverage worked examples

Rankings and corpora below are invented to show the counting rules.

**Redirect target missing (`q009`).** The corpus has a redirect from `Aggression` but not its target page `Aggressiveness`. Both grade-2 judgements are `redirect_target_missing`. Coverage is 0/2. The query is left out of conditional recall. End-to-end recall@k is 0 for every k, and it is reported as a coverage gap, not a ranking failure. `Aggressiveness` appears in the missing-material report.

**One page missing (`q004`).** The corpus has `Rune crossbow` but not `Armadyl crossbow`. Coverage is 1/2. If the engine returns `Rune crossbow > Comparison between other ranged weapons` at rank 1, conditional recall@3 is 1/1 = 1.0 and end-to-end recall@3 is 1/2 = 0.5. The gap between the two figures is the cost of missing material, not of ranking.

**Template-only section.** A judged section exists, but its content is only a template call. It is `template_only`, so it counts against coverage even though the page was found. It goes in the unsupported-content part of the report as a limitation of the imported article. A title is added to the missing-pages part only when the specific template or page it depends on is known and not yet imported.

## Evaluation procedure

1. Build or load the engine's index from a corpus whose kind (`real` or `synthetic`) and import date are recorded. Do not use the benchmark pages to tune the engine and then report the same queries as the result; keep a held-out split if tuning begins.
2. Resolve each query's `query` text through the engine's public retrieval entry point, with a fixed result limit (use 10).
3. Write a run file (below), including timing.
4. Score with the rules above. Assign an availability state to each judgement first, and report coverage, conditional and end-to-end figures together. Exclude `ambiguous` and `unresolved` queries, list them separately, and label every figure provisional while no query has `relevance: reviewed`.
5. Record the engine version, the corpus date, the dataset version and the machine.

Because the Wiki changes with game updates, a judged section may be renamed or removed. A section that is absent from the corpus is reported as `section_missing`, not as a ranking failure. If it was renamed, update the dataset (new `dataset_version`) and keep the old query id's meaning unchanged.

### Run file (proposed, not yet consumed by any code)

```json
{
  "dataset_version": "0.1.0",
  "engine": "name and version",
  "corpus": { "kind": "real or synthetic", "id": "identifier", "imported_at": "date" },
  "results": [
    {
      "query_id": "q001",
      "latency_ms": 12.3,
      "hits": [
        { "page": "Abyssal whip", "section": null, "text_chars": 480 }
      ]
    }
  ]
}
```

`hits` are in rank order. Availability states are not part of the run file: the scorer derives them from the corpus and reports them. This shape may change when the first engine and scorer exist (tracked in #37); any change is recorded here.

## How future implementations use the dataset

- A scorer reads `dataset.v1.json` and a run file and prints the metrics above. When it exists, it should reuse the types currently defined in `tests/eval_dataset.rs` and move them into the library.
- The first keyword engine is run against the dataset to set a baseline. Semantic retrieval or other additions are justified only if they improve the metrics that matter at acceptable cost, as the README principle says.
- Tests that depend on the Wiki must use synthetic fixtures and not this dataset's references, so unit tests stay offline and independent of the live site.

## Extending the dataset

- Add queries with a new `qNNN` id; do not renumber or repurpose ids. Increase `dataset_version` (minor for additions, major when existing meanings change).
- Record every judged page in `pages` with the title observed after redirects, and every judged section in `observed_sections`. If a section cannot be observed, set the query's `references` to `partial` or `unverified` (and `relevance` to `unresolved` if no reference exists) and explain why in `notes`. Grades start as `provisional`; use `reviewed` only after a person has read the section text.
- Keep text short and original. Do not paste Wiki text into any field. The structural test rejects strings longer than 500 characters, but a length bound cannot show that a short string is not copied text, so a reviewer confirms originality by reading the diff.
- Check references from manually supplied exports (where the licensing gate in [issue #6](https://github.com/whcas/OSRS-MCP/issues/6) allows it) or by a person reading the page. Do not fetch Wiki pages with scripts or tools, and do not contact the operator, in line with the human-only acquisition policy in the [investigation](../../docs/investigations/wiki-acquisition.md).
- The 15 to 25 query range checked by the test describes the initial 0.1.0 benchmark. It is not a limit of the format; larger datasets are fine, and the dataset-specific scope checks stay separate from format validation and scoring.

## Limitations

- **Retrieval, not answers.** A system can retrieve the right section and still give bad advice, or retrieve nothing relevant and answer from memory. Answer quality needs a separate evaluation with human or model grading, which is out of scope here.
- **Small and hand-built.** 25 queries, written by one author, cover a narrow slice of the Wiki. Differences between engines on a few queries are noise. Treat results as a smoke signal, not a ranking with statistical weight.
- **Grades are provisional.** They come from page and section titles. No section text was read or human-reviewed, so a high score may reflect the author's title-based guesses. Some page headings were also observed only partially (see `references: partial` and `headings_complete`).
- **Heading text drifts.** Section identity is heading text, which edits can change. Revision fields help but cover only some pages (`oldid` is `null` for several).
- **Observed through a model-assisted fetch tool**, not by reading raw pages in a browser; re-verify before relying on any single reference.
- **Relevance is not safety.** Grades measure whether a section is relevant to a query. Some relevant sections describe risky options (for example `q017` lists a Wilderness course as grade 2). Nothing here implies that retrieval should rank safe options first; advice on risk belongs to the advisor and the account's constraints.
- **Coverage depends on manual selection.** Real-corpus results reflect which pages a human exported, and unexpanded templates and generated tables can hide content. That is why coverage is reported next to ranking quality, and a high conditional score with low coverage says little about the advisor as a whole.
- **Time-sensitive content.** Game updates change both pages and the right answers (for example, rates, requirements and prices).
- **No Hardcore Worlds economy reference.** The Hardcore Worlds page has no heading devoted to the economy, and no other page was identified (`q025`), so that area cannot be scored from the Wiki.
- **Small mode-specific subset.** Only five queries concern Hardcore Worlds, and their grades rest on section titles. The Hardcore Worlds page is recent and may change quickly.
