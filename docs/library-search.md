# Library search

How the library panel's search box finds tracks, what it cannot find today, and
the shape of the fuzzy matching that fixes it.

The operator-facing description lives in [library.md](./library.md#the-library-panel);
this document is the design behind it. Tracked as
[#426](https://github.com/Arskah/radiodiodj/issues/426).

## Today

`Db::search` (`src-tauri/src/library/db.rs`) runs SQLite FTS5 over an
external-content index of **title, artist, album and genre**:

```sql
CREATE VIRTUAL TABLE tracks_fts USING fts5(
  title, artist, album, genre,
  content='tracks', content_rowid='id'
);
```

The query is built by splitting on whitespace and turning each word into a
quoted prefix term, joined by the implicit `AND`:

```
beat abb   →   "beat"* "abb"*
```

So a search is **prefix-AND across all four columns, order-free**. An empty
query skips FTS entirely and lists the table in artist/album/title order. Both
paths cap at 200 rows with no paging, and both filter `missing_since IS NULL`.
Ranking is bm25 (`ORDER BY rank`) with every column weighted equally, unless the
operator has clicked a column header — a chosen sort discards relevance.

The renderer re-queries 250 ms after typing stops
(`src/features/library/LibraryPanel.svelte`).

## What it cannot find

Two distinct gaps, which need two distinct mechanisms. Measured against
_Kraftwerk — Autobahn_:

| Typed           | Found  | Gap                            |
| --------------- | ------ | ------------------------------ |
| `kraf`          | yes    | —                              |
| `auto kraft`    | yes    | —                              |
| `bjork` → Björk | yes    | unicode61 folds diacritics     |
| `werk`          | **no** | prefix only — no infix         |
| `kraftwrek`     | **no** | no typo tolerance              |
| `lodz` → Łódź   | **no** | `ł` is a letter, not an accent |

The `ł` case is worth stating plainly because it looks like a tokenizer setting
and is not: `remove_diacritics 2` does not help, because `ł` decomposes to
nothing. Folding it needs a custom tokenizer or a normalised shadow column, and
is out of scope here.

Two smaller defects, fixed in increment 1:

- **`year` is not indexed** despite `fts5_search_finds_genre_and_year_after_update`
  implying otherwise — that test only asserts the title still matches.
- **Ranking is unweighted**, so a genre hit ranks level with a title hit.

## Why not just swap the tokenizer

A `trigram` tokenizer gives real infix matching (`werk` → Kraftwerk, verified),
costs roughly 3× the index, and is a small diff. But it is still exact-character
matching: it does nothing for `kraftwrek`. It solves the smaller half of the
problem and leaves the half operators actually complain about.

Likewise a subsequence matcher alone is not enough. Benchmarked against
`nucleo-matcher` over a synthetic library, `kraftwrek` scores **zero** hits —
subsequence matching cannot absorb a transposition, because after consuming
`kraftw` the remaining `rek` cannot be found in order in `…erk`.

Typo tolerance requires edit distance. That is the load-bearing finding.

## Shape

Keep FTS5 as the primary path and add a **fuzzy fallback in Rust**, consulted
only when the exact path returns too little:

```
query → FTS5 prefix-AND  ──(hits ≥ threshold)──→  results
                         └─(hits <  threshold)──→  fuzzy pass → exact hits, then loose hits
```

The exact path keeps today's speed and precision for the common case — an
operator typing an artist they know. The fallback only runs on the queries that
fail today, so the hot path is unchanged.

### The fuzzy pass

An in-memory haystack of `(track_id, tokens)` built from the same four indexed
columns, lowercased and split on whitespace. A track matches when **every**
query token matches **some** haystack token, by prefix or within an edit budget:

```rust
toks.iter().any(|h| h.starts_with(q) || dl(q, h) <= budget(q.len()))
```

`dl` is Damerau-Levenshtein (`strsim`), which counts a transposition as one
edit — the `kraftwrek` case. The budget is a function of the **query** token's
length:

| Query token length | Edits allowed |
| ------------------ | ------------- |
| ≤ 2                | 0             |
| 3 – 5              | 1             |
| ≥ 6                | 2             |

Giving 3-to-5 character tokens one edit is deliberate: with a stricter table
`abbey rod` finds nothing, because `road` is one insertion away from `rod`. The
cost is that very short tokens match loosely; ranking absorbs that.

### Cost

Measured on the live library's scale (`docs/rotation.md` records 4295 music
tracks), release build, whole-corpus scan with no early exit:

| Library size | Fuzzy pass |
| ------------ | ---------- |
| 4 295        | 0.4 – 7 ms |
| 20 000       | 10 – 21 ms |
| 100 000      | 42 – 80 ms |

Against a 250 ms debounce there is roughly two orders of magnitude of headroom
at the real size, and the pass is skipped entirely whenever FTS already answers.
No index, no schema change, no migration.

### Keeping the haystack fresh

The haystack is derived state, rebuilt from the database rather than maintained
incrementally. It is invalidated on scan completion, on a metadata edit, and on
a purge — the three places that change an indexed column. At 4295 rows a full
rebuild is cheap enough that incremental maintenance would be the wrong
trade: a stale search result is a bug an operator will not report clearly.

### Ranking and presentation

Exact hits keep their bm25 order and come first. Loose hits follow, ordered by
total edit cost ascending. The two groups are **not** interleaved — an operator
who typed a name correctly should never have a guess pushed above their match.

Whether loose hits are visually marked is an open question for increment 4;
the argument for is that a fuzzy hit arriving silently in a 200-row list looks
like a bug in the exact matcher.

## Increments

Each lands on its own and leaves search working.

**1 — Correctness and ranking.** Quote escaping and the renderer's `catch` are
[#424](https://github.com/Arskah/radiodiodj/pull/424): a `"` in the box reached
FTS5 unescaped, closing the phrase and leaving the trailing `*` in an
unterminated string, and the rejection then travelled into a `void` call that
left the listing silently frozen. The escape lives in `fts5_prefix_term`, which
is also where increment 3 should build its terms. Still open in this increment:
add `year` to the FTS columns and the triggers, or rename the test that claims
it, and weight ranking with `bm25(tracks_fts, 10.0, 8.0, 3.0, 1.0)`. No new
dependency.

**2 — The haystack.** Build and invalidate the in-memory `(id, tokens)` cache,
with no matching behaviour attached and search still answered entirely by FTS.
Lands as pure infrastructure with its own tests, so increment 3 changes matching
against a cache already known to be correct and fresh.

**3 — The fallback.** Add `strsim`, the budget table, the threshold, and the
two-group ranking. This is the increment that changes what the operator sees.

**4 — Presentation.** Marking loose hits, and whatever the threshold turns out
to want after increment 3 is in use.

Increment 2 before 3 is the same reasoning as
[playlist.md](./playlist.md#why-the-refactor-landed-first):
the risky change should land against infrastructure the suite already validates.

**Acceptance criterion for increment 3:** every query in the table under
[What it cannot find](#what-it-cannot-find) returns the track, and no query that
works today returns it lower than it does now.

## Code map

| area                    | where                                            |
| ----------------------- | ------------------------------------------------ |
| query and ranking       | `src-tauri/src/library/db.rs` (`Db::search`)     |
| FTS schema and triggers | `src-tauri/src/library/schema.sql`               |
| search box and debounce | `src/features/library/LibraryPanel.svelte`       |
| renderer state          | `src/shared/state.svelte.ts` (`AppState.search`) |
