# Milestone 4: Deliver

The plan is docs/milestone-4-plan.md. This note keeps what the tasks measured.

## Task 3: claude-mem's history on Design B

`oboete --home ~/.oboete/eval/b-import import claude-mem ~/.oboete/eval/claude-mem-2026-09-24.db --eval-store`, a release build on WSL (ext4), then `oboete rebuild` (every consumer, no AI call).

- **Documents:** 178,370 import ops: 152,030 observations, 13,155 session summaries, 13,185 prompts; 2,100 rows had nothing left to store (empty, or a harness notification). Their source ids are exactly those of v1's evaluation store's `imports` table (178,370 each way, none apart), so the 112-question judgments map through it (D10). The plan's 178,502 counted that store's tables, not its imports.
- **Source name:** the import names the database `claude-mem:b62f07076e19` (its first migration time). v1's evaluation store was built before that naming and calls it `claude-mem`: a mapping from B's uids to its documents goes by source id.
- **Time:** the import took 40 s (46 MB peak memory); the rebuild 60 s (22 MB).
- **Size:** raw.db 323 MB; knowledge.db 864 MB, of which the trigram index is 531 MB and the documents 274 MB. The index keeps no copy of the text (`content=''`); with one, knowledge.db was 1,115 MB. The backup's ops segments are 58 MB.

## Task 4: the search core

`oboete search --all --limit 10 <query>`, a release build on WSL, each query run twice; the first run of a store reads its index from disk (3.8 s on the evaluation home, 1.2 s on the milestone 3 home).

- **claude-mem's history** (`~/.oboete/eval/b-import`, 178,370 imported documents, no claims or records): 0.24 s for "trigram tokenizer" and "Codex review", 0.03 s for "検索の実装", 15-17 MB peak memory. The ten hits are imported documents, each labelled `imported` with its `claude-mem:<project>` repository.
- **A curated home** (a copy of milestone 3's `ov-B1`: 242 claims, 70,904 records): 0.27 s for "worker lock", 0.29 s for "claude-mem import", 24-28 MB. The claims come first, labelled `citable`.

## Task 5: embeddings

Step 13, 2026-10-01: a copy of `~/.oboete/eval/b-import` (178,370 imported documents, no claims or records), a release build on WSL (ext4), the owner's Workers AI account (`@cf/baai/bge-m3`). The copy's `config.toml` raised `daily_requests` for the run (D10), so the 312 dev questions, run twice, and a full day's 160 batch requests fit in one UTC day. The copy was deleted after.

- **MCP search** over stdio, the 312 dev questions of `queries.jsonl` in file order, `all=true`, a warm page cache, the first 10 calls left out of p50 and p95:

  | | p50 | p95 | max |
  |---|---|---|---|
  | full text alone (`provider = "none"`) | 0.44 s | 1.26 s | 1.81 s |
  | the query's call before the full-text sides (the PR's first version) | 0.65 s | 1.56 s | 2.71 s |
  | the call beside them (as merged) | 0.49 s | 1.29 s | 1.91 s |

  The query's call alone took 0.22-0.26 s on average and at most 1.17 s; 5 of the 624 ran past the 1.2 s timeout and those searches fell back to full text. On these long questions the full-text sides take most of the time (Task 4's 0.03-0.24 s were short queries). The MCP server's peak memory was 30-35 MB.
- **A day's batches**: 160 requests (the cap's 200 less the 40 kept for queries) keyed 6,033 documents: 5,972 new vectors and 61 documents mapped to a text already embedded, 37.7 a request, as the 50,000-character budget binds on imported observations of about 1,300 characters. A call took 0.71 s at p50 and 1.37 s at p95; the 159 after the first took 4 min 45 s. The worker's peak memory was 37.5 MB.
- **Neurons against Cloudflare's count** (`aiInferenceAdaptiveGroups` by minute, over those 159 batches): their 1,371,853 estimated tokens were 2,214,962 input tokens and 2,380 neurons by Cloudflare's count, 1.61 times the estimate; the queries, short and mostly Japanese, about 35 tokens each, are near their estimate. The USD a batch reserves now counts 1.61 tokens per estimated token (`COUNTED_PER_ESTIMATED`), which over-counts the queries. A full day of batches is about 2,400 neurons, a quarter of the free 10,000. The account's whole day was 2,644 neurons, v1's included (v1 used 51 and 111 neurons on the two days before).
- **Disk**: 5.2 MB per 1,000 vectors (31.3 MB for 5,972). A 4 KB fp32 vector does not fit a 4 KB page beside its key, so each takes an overflow page (`vectors` holds 27.4 MB of it). All of b-import's vectors would add about 930 MB to its 864 MB knowledge.db.
- **The whole store**, from its texts as they are sent (composed, cut at 12,000 characters): 57.8 million estimated tokens, about 93 million and 100,000 neurons by Cloudflare's count; about 4,700 requests at 37.7 documents each, so about 30 days at 160 requests a day, where the request cap binds and not the neurons, or about USD 1.0 if paid at once past one day's free allowance. v1's embedding of the same history counted 82,657 neurons with its 424 questions (docs/pr-d.md:14). Task 6's Raw corpus is not built yet; Task 6 counts it.
- **A poll's time**: each poll read the imported documents with no key row by scanning and sorting all 178,370 (0.66-0.78 s), and the raw records the same way (0.51-0.59 s over the 70,904 records of milestone 3's base), waiting or not: an idle worker round with the day's requests spent took 1.45-1.49 s, against 0.68 s with embedding off. With the `vector_todo` queue the first round, which makes the queue over the 172,337 documents still waiting, took 1.25 s and the next ones 0.76-0.77 s. Between two batches the worker waited 0.84 s at p50, up to 200 ms of it until it saw the answer; the p95, 4.3 s, came about every 20 batches, which fits SQLite's automatic checkpoint of knowledge.db's WAL (1,000 pages, about 20 batches of vectors here), not traced further.

## Task 6: the test-split run

Step 4, 2026-10-01: `m4.py`'s readers on the owner's transcripts and the frozen stores, read-only (nothing replayed yet).

- **Raw's corpus:** 3,109 sessions, 1,647 of Claude Code and 1,462 of Codex. Left out: 1,280 that started after the evaluation store's copy time (2026-09-24 09:56:46 JST), 142 of claude-mem's observer (run in `~/.claude-mem/observer-sessions`), 138 forked Codex rollouts, 48 run under `/tmp`, the replay set's 30 held-out sessions, and 5 transcripts with no time. A session that runs past the copy time stays, its later events left at the replay. The late count grows as the agents run.
- **Forked Codex rollouts:** each of the 138 opens with a copy of its parent's history (29 parents; every fork shares its parent's typed prompts), which the converter would replay as the fork's own records, though the live hooks never send it again. None of the parents is a test question's session or a held-out one, so leaving them out changes no question's own-session rule today; it keeps the copies out of the store, with the forks' own later turns.
- **The agents' windows**, from the first event day of every transcript (JST): Claude Code from 2026-06-19, Codex from 2026-08-23. Raw's N is 25 of the 165 test questions (22 of the 112, 3 of M21's 53), as the plan counted.
- **The v1 store:** 178,370 `imports` rows, one per B uid by source id, and 132 documents v1 wrote itself with none (D10's o1-o106, s1-s14, p1-p12).

Step 8, 2026-10-01: the baselines.

- **v1:** a release build of the `v1` branch at 6f70317 (SHA-256 32f65a0bbb5bbd6fa76ac73d1e9394834ec05503bd250f83d6306846ccf005bd) on a copy of `~/.oboete/eval/home`: `search x`, `load_vectors.py` (178,502 vectors), `reindex` (no request left). On the 112 test questions, `eval --method fts` and `hybrid --depth 50` gave D2's frozen files exactly, every question's top 50. The 53 English questions' lines were appended to D2's files cut to the 112: `runs-test-m4/hybrid-d2.trec` (SHA-256 eb1923a9919c428f9537c0d3d63ee0c8f88c849e03657cb5cc7d43ea3ed0f8d1) and `e0-trigram.trec` (20cefcac4fd0295411f87e99d9bdf8e3335d8023246a067dd341210096f8ff00), 165 questions each. The copy was deleted.
- **claude-mem 13.28.0**, both runs on 2026-10-01 with `run_claude_mem.py --questions --out`: `claude-mem.trec` (5,316 lines; 2,487 ids newer than the copy dropped; SHA-256 aa3e98319efac30f8270b5a029154c7671f85cb9fcf0e59a1ceebdb1e441be56) and `claude-mem-nowindow.trec` (6,460 lines; 1,790 dropped; dbf5fc6b6ac09f18c930d356fc37e1920ab72899456e4d63ac4c392dd2a68475), 165 questions each, no query failed.

Step 9, 2026-10-01: the evaluation home `b-m4`, a copy of `b-import` with `[summary] curate = false` and no embedding provider.

- **Replay:** `m4.py corpus` (3,109 sessions; 324,943 events before the copy time, none failed to convert; 44 sessions have none), then `m4.py replay` with a release build of 33e7c9b (`~/.oboete/eval/bin/oboete-33e7c9b0adb3`, SHA-256 ce6c9f28c329b8857f45aa1b6ac38a3f0e6d005bcb4a7b5d8b92e14aa861fc53) and `oboete worker --idle-ms 5000` until it drained: 30 min 39 s, 99 MB peak, no worker error. Every consumer's checkpoint is at raw.db's highest seq, 325,577; 178,370 ops; `raw_docs` holds 324,943 records (302,052 of them tool outputs); no providers.db, no claim. The home is 8.5 GB.
- **Raw's N** is 25, and 8 of those questions (5 sessions) are of the replay set's held-out sessions, which the corpus leaves out: they count with no record of their own session. The gate names them apart and checks every other counted question's session for a record (D10 as amended here); the pre-registration lists them.
- **Embedding** waits for the owner (What needs the owner, item 8): Raw's corpus is about 542,000 neurons (22,726 requests, 98% of it tool output) and the imported history about 100,000, together about USD 7 at once or about 64 days inside the free allowance.
