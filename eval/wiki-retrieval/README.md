# OSRS Wiki retrieval evaluation dataset

A small, versioned benchmark for comparing future keyword and semantic retrieval over OSRS Wiki sections. It measures **retrieval** only: whether the right article sections come back, how much text comes back and how fast. It does not measure whether a model's final advice is correct (see [Limitations](#limitations)).

No retrieval engine, index or Wiki download exists yet, so this directory holds the dataset, its format and the evaluation procedure, and no results.

## Files

| File | Purpose |
| --- | --- |
| `dataset.v1.json` | Queries, relevance judgements and the page records they refer to |
| `README.md` | This document: format, metrics, procedure, limitations |
| `../../tests/eval_dataset.rs` | Offline structural checks, run by `scripts/check.sh` |

Dataset content is original: query wording is written for this project, and the references are page titles, section heading names and revision identifiers. No Wiki prose is copied, which keeps the file clear of share-alike questions (see the [acquisition investigation](../../docs/investigations/wiki-acquisition.md), if merged, and issue #6). No personal data or account exports are included.

## Format (`format_version` 1)

Top-level fields:

| Field | Meaning |
| --- | --- |
| `format_version` | Integer. Changes only for breaking format changes |
| `dataset_id`, `dataset_version` | Identity of the benchmark, and a semantic version for its content |
| `created` | Creation date (`YYYY-MM-DD`) |
| `source` | Site, observation date, method used to check references, and a licence note |
| `grades` | Meaning of relevance grades `1` and `2` |
| `pages` | Page records referred to by judgements |
| `queries` | The benchmark queries |

### Page record

| Field | Meaning |
| --- | --- |
| `title` | Canonical page title after redirects, as observed |
| `requested_as` | Redirect source titles that were requested and resolved to this page |
| `oldid`, `last_modified` | Permanent revision id and date, both `null` when not observed |
| `revision_basis` | Where the revision came from: `footer` (page footer), `summary` (fetch-tool summary), `unspecified`, or `not_observed` |
| `headings_complete` | `true` if the full heading list was seen; `false` if the page body was only partly read |
| `observed_sections` | Section paths seen on the page |
| `note` | Optional caveat |

A **section path** is heading texts from the top level down, joined by ` > `, for example `Walkthrough > Preparing for the fight`. A section path names a section by heading text, which can change between revisions; the revision fields say which text was observed.

### Query record

| Field | Meaning |
| --- | --- |
| `id` | Stable identifier `qNNN`; never reused |
| `category` | `exact_name`, `multi_entity`, `game_mechanic`, `quest`, `skilling`, `hardcore_safety` or `money_making` |
| `query` | The text given to the retrieval system |
| `intent` | Why the query is in the set |
| `status` | See below |
| `tags` | Free labels such as `redirect`, `lexical_gap` (the query avoids the page's own words), `cross_page`, `numeric_range`, `exploratory` |
| `judgements` | List of `{page, section, grade}`. `section` is a section path, or `null` for the page as a whole |
| `notes` | Required for `partial`, `ambiguous` and `unresolved` queries |

**Grades:** `2` means the section directly answers the query; `1` means useful supporting context. Anything not listed is treated as not relevant. Grades are author judgements made from page and section titles, not from reading section text. The checks below show a reference exists, not that its text is correct. A human review of the grades is expected before results are used for decisions.

**Status:**

| Status | Meaning | Scored by default |
| --- | --- | --- |
| `verified` | Every judged page and section was observed on the Wiki | Yes |
| `partial` | Pages exist, but headings were read only partly, or relevance rests on little evidence; see `notes` | Yes |
| `ambiguous` | Several defensible answers, or the ground truth lies in pages not checked | No; report separately |
| `unresolved` | No reference could be established | No; report separately |

Ambiguous and unresolved queries are kept so the gaps stay visible, rather than inventing ground truth.

### Coverage

25 queries: 3 exact-name, 3 multi-entity, 5 game-mechanic, 3 quest, 3 skilling, 5 Hardcore safety and 3 money-making (one `ambiguous`, one `unresolved`). Several deliberately test redirects (`fairy rings`, `hardcore ironman`), wording that avoids the page's own terms (`lexical_gap`) and answers spread over more than one page.

## Matching rules

A retrieval result is `(page, section path)`, with the page title resolved through redirects to the canonical title in the dataset.

- **Section match:** the result matches a judgement if it is on the same page and its section path equals the judged path or is a descendant of it. A judgement whose `section` is `null` matches any section of that page.
- **Page match:** the result is on the same page as the judgement, regardless of section. Use this for engines that return whole pages.
- A result that is an ancestor of the judged section (a larger chunk containing it) counts as a page match only.

## Metrics

Compute each metric over the scored queries (`verified` and `partial`) and report it per category as well as overall. Relevant items for a query are its judgements with grade `2`, unless stated.

| Metric | Definition |
| --- | --- |
| Recall@k (k = 1, 3, 5, 10) | Fraction of the query's grade-2 judgements matched by at least one of the top k results. Report at both section level and page level |
| Hit@k | 1 if any grade-2 judgement is matched in the top k, else 0 |
| Reciprocal rank | 1 / rank of the first result matching any grade-2 judgement, 0 if none. The mean over queries is MRR |
| nDCG@10 (optional) | Using grades 1 and 2 as gains, with section-level matching, each judgement counted once |
| Response size | Characters (and, when available, model tokens) in the text returned for the top k results; report mean and the largest |
| Latency | Wall-clock time per query. Report the median and the 95th percentile, separately for cold start and warm runs, with the measurement machine described |
| Index metadata | Corpus revision or fetch date, index size and build time, recorded once per run |

A single number across all metrics is deliberately not defined; compare engines on quality and cost together.

## Evaluation procedure

1. Build or load the engine's index from a corpus whose revision or fetch date is recorded. Do not use the benchmark pages to tune the engine and then report the same queries as the result; keep a held-out split if tuning begins.
2. Resolve each query's `query` text through the engine's public retrieval entry point, with a fixed result limit (use 10).
3. Write a run file (below), including timing.
4. Score with the rules above, excluding `ambiguous` and `unresolved` queries from headline figures and listing them separately.
5. Record the engine version, the corpus date, the dataset version and the machine.

Because the Wiki changes with game updates, a judged section may be renamed or removed. When a run reports a miss, check whether the judged section still exists in the corpus before treating it as a retrieval failure; update the dataset (new `dataset_version`) and keep the old query id's meaning unchanged.

### Run file (proposed, not yet consumed by any code)

```json
{
  "dataset_version": "0.1.0",
  "engine": "name and version",
  "corpus": "revision or fetch date",
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

`hits` are in rank order. This shape may change when the first engine and scorer exist; any change is recorded here.

## How future implementations use the dataset

- A scorer reads `dataset.v1.json` and a run file and prints the metrics above. When it exists, it should reuse the types currently defined in `tests/eval_dataset.rs` and move them into the library.
- The first keyword engine is run against the dataset to set a baseline. Semantic retrieval or other additions are justified only if they improve the metrics that matter at acceptable cost, as the README principle says.
- Tests that depend on the Wiki must use synthetic fixtures and not this dataset's references, so unit tests stay offline and independent of the live site.

## Extending the dataset

- Add queries with a new `qNNN` id; do not renumber or repurpose ids. Increase `dataset_version` (minor for additions, major when existing meanings change).
- Record every judged page in `pages` with the title observed after redirects, and every judged section in `observed_sections`. If a section cannot be observed, set the query's status to `partial` or `unresolved` and explain why in `notes`.
- Keep text short and original. Do not paste Wiki text into any field. The structural test rejects fields longer than 500 characters.
- Check references through the live Wiki only in small numbers, with a descriptive User-Agent and no bulk fetching, and follow the open permission question in the acquisition investigation.

## Limitations

- **Retrieval, not answers.** A system can retrieve the right section and still give bad advice, or retrieve nothing relevant and answer from memory. Answer quality needs a separate evaluation with human or model grading, which is out of scope here.
- **Small and hand-built.** 25 queries, written by one author, cover a narrow slice of the Wiki. Differences between engines on a few queries are noise. Treat results as a smoke signal, not a ranking with statistical weight.
- **Grades come from titles.** Section text was not read when grading, and some page headings were observed only partially (see `partial` queries and `headings_complete`).
- **Heading text drifts.** Section identity is heading text, which edits can change. Revision fields help but cover only some pages (`oldid` is `null` for several).
- **Observed through a model-assisted fetch tool**, not by reading raw pages in a browser; re-verify before relying on any single reference.
- **Time-sensitive content.** Game updates change both pages and the right answers (for example, rates, requirements and prices).
- **No Hardcore-economy reference.** The Hardcore-world market has no Wiki page identified here (`q025`), so that area cannot be scored from the Wiki.
