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

## Task 7: the viewer on Design B

Step 7 (row 53-6), 2026-10-01: a copy of `~/.oboete/eval/b-import` (178,370 imported documents, no claims, records or vectors, no embedding provider, so search answers from full text), a release build of the server side (9686a62) on WSL (ext4), started in this repository's checkout, deleted after. Each route ran in a fresh viewer: the first request after the copy's files were dropped from the guest's page cache (`posix_fadvise`; the Windows host may still hold them), then one pass discarded and 40 requests over the route's variants (the first 20 dev questions of `queries.jsonl` for search, 10 imported documents from a search for doc). Peak memory is the viewer's VmHWM.

| route | first request | p50 | p95 | peak memory |
|---|---|---|---|---|
| `/` (the page) | 0.001 s | 0.001 s | 0.001 s | 8 MB |
| search, the checkout's repository | 2.77 s | 0.13 s | 0.56 s | 27 MB |
| search, `all` | 3.21 s | 0.26 s | 1.05 s | 28 MB |
| search, `all`, `raw=only` (no records) | 0.009 s | 0.002 s | 0.003 s | 14 MB |
| doc | 0.006 s | 0.002 s | 0.003 s | 14 MB |
| timeline, the checkout's repository | 1.98 s | 0.09 s | 0.10 s | 16 MB |
| timeline, `all` | 2.87 s | 0.27 s | 0.33 s | 17 MB |
| timeline, `all`, the second page (`before`) | 2.93 s | 0.27 s | 0.29 s | 16 MB |
| context | 0.007 s | 0.002 s | 0.003 s | 14 MB |
| repos | 1.96 s | 0.18 s | 0.21 s | 20 MB |
| version | 0.006 s | 0.002 s | 0.002 s | 13 MB |
| stats | 0.006 s | 0.002 s | 0.002 s | 14 MB |

No route's p95 passes 1.5 s, so none gets an index. A route's first request reads its index from disk (2-3 s, as Task 4's first search did). Search over records and vectors is the search core's, measured by Tasks 5 and 12b, not here.

## Task 8: per-prompt injection

Step 11, 2026-10-01: the threshold. A copy of milestone 3's `ov-B1` (the dev base curated over 70 pair spans, 89 claims under `claims::DECIDED_WHERE` in five repositories), a release build of `m4/inject`. Each of the base's 465 typed prompts in those repositories (harness envelopes left out) was scored against the delivered claims said before it, since a claim said later, its own source included, was not there to inject: the share of the prompt's trigrams a claim's body holds (`shortlist::pick`). Every prompt whose best claim scored 0.25 or more (34) was judged by hand: whether that claim bears on the prompt. The prompts are the owner's, so the script's output and the judgments stay with the evaluation's owner-only files (`~/.oboete/eval/inject-step11.json`). `oboete inject --prompt` printed the same shares on the prompts checked.

- **Short prompts.** A prompt of a few trigrams tells no topic: three that no claim bears on, of 4 to 7 trigrams, scored 0.43, 0.60 and 0.75, so a plain share would need a threshold past 0.75, which 3 of the 22 answered prompts reach. A text's trigrams now count as at least 10 (`shortlist::MIN_GRAMS`, set before the threshold).
- **`THRESHOLD` = 0.53.** The best-scoring prompt no claim bears on scored 0.529 (it shares a tool's name with an open item about another hook). At 0.53, 8 of the 22 prompts a claim bears on get it, and one of the 8 also gets two claims that only share that tool's name: the share cannot tell them apart. The plan asked for 20 and 20: the 12 unanswered prompts that scored 0.25 or more were all judged and every other prompt scored less, so the threshold holds over all 465; the 22 answered are all that scored 0.25 or more, so answered prompts below were not counted (none would pass).
- **Drafted questions.** 20 questions a later session might ask about 20 seeded delivered claims, written in the owner's style without the claims' words: 3 pass 0.53, and the others' claims scored 0 to 0.53 (median 0.2). The share finds a claim when a prompt repeats its words, not a paraphrase, so per-prompt injection stays off by default (D9) until Task 12b's Inject harness measures it.
