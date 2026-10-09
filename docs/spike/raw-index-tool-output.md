# Spike: how much of each tool output the raw index keeps (#317, second step)

Throwaway (spec 8.3). Issue #317's second step, after the first took the index's own copy of the text out (docs/spike/raw-index-copy.md, #417). Run 2026-10-09.

## Question

Tool output is nearly all of the records' text: of September's 892 MB indexed on the copy of the owner's home measured in #317, 869 MB came from 142,429 tool records, against 23 MB from the 9,722 others (prompts, replies, envelopes, compactions). So it is nearly all of `raw_fts` (1,990 MB of trigram index that month) and of the records' vectors, which the embedding phase makes for every record (milestone 4's D8). How much smaller is the index when it keeps only the head and tail of each tool output, or none of it, and how much of what the index returns today does each keep?

## Harness

`raw-index-tool-output/tool_output.py RAW_DB QUERIES OUT_DIR [EVERY]`, read only, on the raw.db of the evaluation home `b-m4` (milestone 4's Raw corpus, 325,577 records): one record in five, 64,988 records holding 324 MB of text as `fts::text` makes it, 97% of it tool output. Each variant indexes that text in an FTS5 trigram table without a copy, as `raw_fts` is since #417, with each tool record's text cut to its first and last characters as the variant names. The queries are the evaluation's dev questions (312, each with a trigram), matched as `search::trigrams` makes them, top 10 by bm25. For each variant it measures the index's size and how much of the whole index's top 10 it still returns. Tombstones are not applied: sizes are the point, and the home's 634 tombstones move neither number. It also builds, once, the index #419 would add for two-character words (each record's distinct pairs with a Han or katakana character, `detail=none`). It prints aggregates only: `raw-index-tool-output/result-every5.txt`, from `python3 tool_output.py ~/.oboete/eval/b-m4/raw.db ~/.oboete/eval/queries.jsonl /dev/shm/oboete-317-step2 5` (Python's `zstandard` module reads the bodies).

Vectors were not built again: milestone 4's Step 13 measured 5.2 MB per 1,000 (docs/milestone-4.md).

## Results

| tool output kept | index MB (sample) | of the whole | top 10 kept, mean | queries losing a top-10 hit | lost hits: tool / other |
|---|---|---|---|---|---|
| all of it | 713 | 100% | 100% | 0 of 312 | 0 / 0 |
| first 16 KB and last 4 KB | 606 | 85% | 91% | 179 of 312 | 246 / 21 |
| first and last 4 KB | 471 | 66% | 79% | 270 of 312 | 603 / 42 |
| first and last 1 KB | 271 | 38% | 53% | 306 of 312 | 1,371 / 105 |
| none | 27 | 4% | 17% | 312 of 312 | 2,576 / 26 |

The whole index's top 10 is mostly tool output, so every cut loses hits, and the hits it loses are nearly all tool records: the few others lost moved because the cut changed bm25's statistics. A head and tail saves less than it costs: keeping 4 KB at each end, a third of the index goes and a fifth of the hits with it.

At September's rate (the copy measured in #317), with tool output out:

- The index: about 4% of the 1,990 MB, about 75 MB, plus `raw_docs`, which keeps a row for every record (15 MB): about 0.09 GB a month, instead of about 2.2 GB.
- The vectors: 9,722 records, about 0.05 GB a month, instead of about 0.8 GB for all 152,151.
- A word too short for a trigram is found by reading raw.db's bodies (`fts::holding`, docs/spike/raw-index-copy.md); the scan now reads only the records the index holds, about 3% of the text it read.

#419's index of two-character words would be 8 MB here, 1% of the whole trigram index (3,527,465 distinct pairs, tool output included).

## Decision

Tool output is not searched (spec, owner decision 42). A tool record stays in raw.db with its `raw_docs` row, and `get` returns it by its key; neither `raw_fts` nor the vectors take it in, and the search over records reads no tool record, the short-word scan included. claude-mem does the same: its full-text tables are its observations, session summaries and memory items, its vectors are its observations, session summaries and prompts, and the tool input and output it keeps (`tool_uses`) are returned only by id (`get_tool_uses`, 13.34.2 and 13.35.0). What a tool output holds reaches search through the cards and summaries curated from it.

The other choices shown to the owner, and why not:

- All of it, about 3 GB a month of index and vectors: the growth #317 was opened for.
- The last 30 days: the index stays near a month's size, but a day's tool output leaves it every day, and a contentless-delete table's bm25 totals keep counting each deleted row until `oboete rebuild` (docs/spike/raw-index-copy.md), so it needs a periodic rebuild as well as a setting.
- A head and tail of each output: the table above.

Whether search over tool output helps is left to the one evaluation after completion (decision 34). If it does, a window of recent tool output can come back: raw.db keeps all of it, and `fts::indexed` is the one rule to change.

A home indexed before keeps its tool rows in `raw_fts`, and any vectors made for them, until `oboete rebuild`; the rebuild still carries those vectors' blobs in its cache of embedded texts, unreferenced, as it carries every text ever embedded (`embed_phase::carry`). Search reads past them: the full-text side leaves tool records out, and `raw_rows` drops one whichever side found it. A reindex of one (a tombstone over it) takes its row out of `raw_fts`, and the embedding queue drops one it still holds unsent.
