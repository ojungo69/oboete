# Spike: the raw index's own copy of the text (#317)

Throwaway (spec 8.3). Issue #317; the owner's decision of 2026-10-09: fixed before the switch, the step with no recall loss first. Run 2026-10-08 and 2026-10-09.

## Question

`raw_fts`, the FTS5 trigram index of every record in knowledge.db, kept its own copy of each record's text (`raw_fts_content`): 997 MB of knowledge.db's 3.17 GB for September's records on the copy of the owner's home measured in #317. raw.db already holds the same text, zstd-compressed. What does dropping the copy cost the readers that used it (a query too short for a trigram, snippets, the embedding phase's raw documents), and does a forget still leave nothing behind in knowledge.db?

## Harness

`docs/spike/raw-index-copy/`, one synthetic corpus throughout: public crate sources under `~/.cargo/registry/src` standing in for tool output (196 MB in about 26,700 records of 0.8 to 120 KB), a rare two-character word (楓樹) in 30 records spread over the history, a common one (設計) in every 20th record.

- `bench.py OUT MB`: the corpus zstd-compressed in a raw.db, and three knowledge.db layouts built from Python: `copy` (as before), `none` (contentless, `content=''` with `contentless_delete=1`, as `imported_fts` already is; the text read back from raw.db) and `compressed` (contentless plus a zstd copy in knowledge.db, read through a function). It times a trigram query, a short rare and a short common query over every record, and snippets of 20 hits, then forgets 1,000 records and looks for their canary in knowledge.db's bytes. Results: `bench-300-results.txt` (asked for 300 MB; the sources hold 196).
- `fast.py RAW_DB`: the short query over raw.db's bodies with a byte pre-filter before a body is parsed, as `fts::holding` does.
- `forget2.py DIR`: whether a deleted row's trigrams stay in the file, `copy` and `none`.
- `rust.py OUT MB copy=<bin> none=<bin>`: the same corpus replayed as Codex PostToolUse records into one home per build (release builds of main at 82bebe7 and of this branch), indexed by `oboete worker`, searched from the command line (`oboete search --all --raw only --limit 20`), median of 7. The homes send nothing: no providers, curation off, no embedder. Results: `rust-results.txt`.

## Results

The SQLite layouts (bench.py; raw.db 55 MB):

| layout | knowledge.db MB | build s | trigram ms | short rare ms | short common ms | 20 snippets ms | canaries left |
|---|---|---|---|---|---|---|---|
| copy | 577 | 22 | 4.9 | 163 | 159 | 0.3 | 0 |
| none | 366 | 21 | 5.0 | 484 | 72 | 0.9 | 0 |
| compressed | 419 | 22 | 4.9 | 230 | 4.5 | 0.6 | 0 |

Through oboete (rust.py; raw.db with its WAL 209 MB in both homes):

| build | knowledge.db MB | index s | trigram ms | short rare ms | short common ms |
|---|---|---|---|---|---|
| main (copy) | 559 | 40 | 347 | 170 | 292 |
| this branch (none) | 365 | 40 | 351 | 748 | 243 |

The command-line times include starting the process. Both builds return the same 20 hits for each query (`rust-results.txt`; device ids, times and the home's path set aside). A first run gave the branch 649 ms for the short rare query.

Run again on 2026-10-09 (`rust-results-2.txt`: the registry had gained a crate, 26,834 records of 197 MB), then the short queries alone, median of 9, with the branch's final scan beside the one above: it makes no lowercase copy of a body for a word with no ASCII letter, and no second copy of a body that is valid UTF-8.

| query, ms | main (copy) | branch, above | branch, final |
|---|---|---|---|
| 楓樹 (short rare) | 177 | 657 | 410 |
| 設計 (short common) | 271 | 214 | 196 |
| M5 (short, ASCII) | 858 | 1,323 | 1,105 |

In that run the trigram query's 20 hits were the same but two near neighbours traded places (8th and 10th). FTS5 does not lower a contentless-delete table's totals, its rows and tokens, when a row is deleted: 54 records of the corpus had a range masked by the rescan after they were indexed, and each was deleted and indexed again, so bm25 counted 26,888 rows for 26,834 and the tokens of the masked texts twice. SQLite 3.45 with three rows shows the same: a delete leaves the totals of a `contentless_delete=1` table as they were, and lowers those of a normal one. `imported_fts` has had the same since it was made contentless.

A forget (forget2.py): how often each trigram of the deleted row is in the file's bytes.

| layout | before | deleted, VACUUM | `optimize`, VACUUM |
|---|---|---|---|
| copy | 3 | 2 | 0 |
| none | 1 | 1 | 0 |

## Decision

`raw_fts` is contentless with `contentless_delete=1`, as `imported_fts` is. knowledge.db is 35% smaller here, and on #317's measurement of September (3.17 GB, of which the copy was 997 MB) the index adds about 2.2 GB a month at the owner's rate instead of 3.2 GB. Readers read the text back from raw.db: snippets, the evaluation's sidecar and the embedding phase through `Raw::at` and `fts::texts`, and a word too short for a trigram through `fts::holding`, which skips a body that cannot hold the word before parsing it. A home made before keeps its copy until `oboete rebuild`; nothing reads it.

What it costs: a query made only of short words that are rare in the store reads raw.db's bodies newest first until it has its hits, 410 ms where the copy took 177 ms on 197 MB, and both grow with the store. A query with a trigram, and a short word beside a longer one, are as fast as before, as the trigram index finds the rows first; a common short word stops at its first hits. The `compressed` layout would keep the short scan near the copy's for 15% more knowledge.db and a second copy that a forget must also reach; it is not built. And bm25's totals keep counting each record a forget, a mask or a rewind took out of the index: near ties move, the hits do not, until `oboete rebuild` makes knowledge.db again.

Shown to the owner on 2026-10-09, who left the choice to the long term (「長期的に見て良い選択肢を判断して」): no copy. A copy only shortens a scan that grows with the store either way, at about 1 GB a month; the long-term answer for short words is an index of them, which needs no copy (#419).

A forget: FTS5 leaves a deleted row's trigrams in its segments, with the copy or without it, until the table is optimized. Milestone 5's physical purge (Slice 2) therefore runs `optimize` on `raw_fts` and `imported_fts` before VACUUM (docs/milestone-5-plan.md).

Next, #317's second step: how much of each tool output the index keeps (its options 2-4), decided after a measurement of the trade-off is shown to the owner.
