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

## Task 9: the cut-over tools

Step 9, 2026-10-01: the owner's v1 store copied with `sqlite3 -readonly ~/.oboete/oboete.db ".backup <dir>/oboete.db"`, its config.toml beside it, migrated into a temporary home under the scratch directory by a release build of 8577a2f on WSL (ext4), all deleted after. The copy held 64,608 events (its highest id is 64,608: v1 never deleted one), 123 sessions, 110 `session_repos` rows, 3,217 observations, 492 summaries and 319 prompts.

- **`oboete migrate`:** 64,608 `oboete-v1` records, 110 `touch` labels and 4,028 import ops; 53.3 s, 49 MB peak memory. raw.db is 277 MB, its bodies plain until the worker's compression (v1's store is 302 MB).
- **Run again:** nothing imported (the 4,028 documents seen before), 0.13 s, 16 MB.
- **v1's store:** its SHA-256 is the same after both passes and doctor. At the first read-only open SQLite made an empty -wal and a 32 KB -shm beside the copy (the `sqlite3 -readonly` count did too), as v1's own hooks keep them while it runs.
- **Settings:** the copied config.toml (`gemini` and `[embedding]`) went into the home unchanged; migrate printed `[summary] curate` as not set, and `[inject]`, `[capture]`, `[redaction]` and `[chain]` as at their defaults.
- **doctor:** "v1 events not migrated yet: 0", and the old store listed until `--finish`.
- **`oboete import transcripts`, the preview** (a release build of c5fe8fe, on the copy's home, which has no raw.db): every transcript on this machine. Claude Code: 3,009 files, 2,963 sessions, 283,277 events (1.05 GB of bodies), 46,513 cut, 406 housekeeping sessions. Codex: 1,856 files, 1,836 sessions, 94,653 events (981 MB), 5,047 cut. None masked or waiting; 11 min 29 s, 151 MB peak; nothing was created in the home. Its cut reads v1's store only, so on a home that records already it is an upper bound.
- **`--yes`, on September's transcripts** (Claude Code's last written in September and Codex's `sessions/2026/09`, hard-linked into a scratch root, imported into a copy of the migrated home): Claude Code 1,843 files, 1,829 sessions, 109,759 events imported (434 MB), 38,288 cut at their session's first v1 record, 406 housekeeping sessions; Codex 1,254 files, 1,240 sessions, 47,631 events (587 MB), 3,865 cut. 5 min 37 s, 153 MB peak. Run again: nothing imported (148,047 and 51,496 events already past), 34 s.
- These transcript counts include the parser's own SessionEnd, one per session, which 842b08c no longer imports (a session resumed after an import would have lost its next event).
- The current transcript import also excludes synthetic end-of-file events: a dangling tool call and a final Stop without a turn-end record. A transcript checkpoint from before prefix verification and this filtering refuses a resume; recovery imports the source again into a fresh home with `--home <new-directory>`.
- **The worker on that home** (a debug build of 842b08c, `curate = false`, no embedder; no providers.db was made, so nothing was sent): 39 min, 82 MB peak. The transcript records' 1.02 GB of bodies are 359 MB of zstd (153,404 records; 3,986 too small to gain stay plain); raw.db is 1.42 GB on disk, 842 MB in use. knowledge.db is 3.17 GB: the raw index's trigram data 1,990 MB and its copy of the text 997 MB, 97% of that text tool output (issue #317). The home was a copy, so its v1 records are under the old device, which the record consumers leave to milestone 6's sync: they stayed plain and unindexed here.

## Task 12a: the dev harnesses' protocol (D12)

Fixed on 2026-10-01 in this commit, before any M6, M5, MUST-M11 or per-kind run reads a result (plan Task 12a Step 2). A later change is a dated subsection below, written before the run it applies to and naming what changed and why. Dev runs decide nothing (spec 8.1); the deciding runs on the held-out transcripts use this protocol after A88.

### Sets and the dev home

- **Sessions.** The replay manifest's 30 `dev` sessions (`~/.oboete/eval/replay/manifest.json`, seed `oboete-milestone-1-2026-09-26`: 24 Claude Code, 6 Codex), the sessions v1's `baseline/oboete-5fa472f04ab6` holds, so both M6 arms hold the same sessions. The 112 test questions are not read; a held-out session only under the guard below.
- **The dev home.** A fresh `m3.py replay` of the 30 sessions with a release build of `main` (its SHA-256 recorded). M6's keys are mapped to its records before any worker run. It is then curated whole by `oboete worker --idle-ms 0` with `[summary] curate = true` and m3.py's `LIVE` entry (Claude Haiku through the owner's subscription), the config written as `stub()` writes it; `live()` and `spans()` are not used. No embedder: its runs are full text, recorded with `Answer.vector` off. It is the Window sweep's point at the default window size, reused and never curated again; its curation report (windows and their outcomes, claims per kind, provider calls) is written beside it.
- **Files.** Questions, keys, answers, labels and every model call stay in owner-only files under `~/.oboete/eval/m6/` and `~/.oboete/eval/m5/`, as milestone 3's labels do. Each model call is kept with its model and the SHA-256 of its prompt, so a stopped run resumes without asking again and `score` asks nothing a run asked.
- **Each run records** N, the machine, the binary's hash, the home, the vector side, and every model with the name its provider reports.

### Models

- **Answerer**: `claude-sonnet-5` through `claude -p` with judge.py's isolation (`common.claude_json`: no tools, hooks, MCP servers or kept session, `clean_env`), the same for every arm.
- **Key writer** (M6) and **labeller** (M5): one panel judge per item (spec 8.1's eight), drawn by `h(f'm6-key:{SEED}:{qid}')` or `h(f'm5-label:{SEED}:{session}')`. **Checker**: the 20% of items with `h(f'm6-check:{SEED}:{qid}') % 5 == 0` (`m5-check` for M5), checked by a judge drawn by `h(f'm6-checker:{SEED}:{qid}')` (`m5-checker`) from the other seven.
- **Graders** (an answer correct, a span holding the answer, an item shown as open, a claim's kind): three panel judges of three makers, none the answerer's, fixed here: `gpt-oss-120b` (OpenAI, through Groq), `deepseek-v4-pro` (DeepSeek) and `glm-5.3` (Zhipu), the last two through OpenCode Go. Their majority decides. Nothing is scored until all three have answered; a judge that has not answered after `calib.chat`'s retries is asked again on the next run. Each grader's agreement with the other two is reported.
- Every panel call goes through `calib.chat` (temperature 0). `SEED` is common.py's.

### M6: lookup

- **Questions** (`questions-dev.jsonl`, drafted by Claude from the dev transcripts; no arm is searched while drafting). 40 lookup questions ("how did we fix X", "which command does Y", "what did we decide about Z"), each:
  - answered in one dev session's records (a prompt, a reply or a tool output), whose session and record the drafter writes down;
  - specific to the work (a file, a command, a value, a decision), so general knowledge does not answer it;
  - in the language the owner used in that session;
  - asked as of `asked_at`, one time for all 40: the day after the last dev event;
  - left out when a later dev session changes its answer;
  - at most two per session.

  About 8 carry a date phrase, an absolute date or one relative to `asked_at` ("last week"), whose range holds the source session's day: MUST-M12's dated subset.
- **Keys** (`m6.py keys`, `keys-dev.jsonl`). The key writer reads the question with its source record and the two records either side, each through `oboete gate` and cut at 4,000 characters (judge.py's window), and writes the answer and the records that state it (KEY). The checker reads the same and the key, and agrees or says why not (CHECK). A key the checker rejects goes back to the drafter, who fixes or drops the question before any run; the agreement rate is reported. Each key's records are mapped to the dev home's records (`device:seq`) before any worker run.
- **Run** (`m6.py run --arm b|v1|b-cmem`). Per question, call 1 (QUERY) turns the question and `asked_at` into `{query, since, until}`. The harness runs `oboete search --all --limit 10 <query>`, with `--since` and `--until` when given, then `oboete get` on the top three hits. Call 2 (ANSWER) answers from those results only, citing their ids, or says they do not answer. The results are the search's lines as printed and the three `get` texts, each cut at 4,000 characters. The arms:
  - `b`: the dev home, a release build of `main`;
  - `v1`: `baseline/oboete-5fa472f04ab6` and a binary built from the `v1` branch, its SHA recorded; v1's search has no time filter, so its `since` and `until` are dropped and counted;
  - `b-cmem`: a copy of the dev home with `oboete import claude-mem --eval-store`, for MUST-M13's zero only, not compared with v1.
- **Scoring** (`m6.py score`).
  - An answer is correct when the graders' majority says it gives the key's answer (CORRECT).
  - A cited claim's spans are its evidence rows from `oboete cite`; a row is valid when `live` and the graders' majority says its quote holds the key's answer (HOLDS). A cited raw record's span is its text as `get` prints it, valid when the graders say it holds the answer. A v1 prompt is a raw record.
  - Attributed-only hits (quote-only claims, imported documents, v1's observations and summaries) have no span: they count toward no answer, and answers that cite only them are reported apart.
  - An answer counts when it is correct and at least one hit it cites has a valid span.
  - Validity: valid spans over all spans of the citable hits cited, at least 0.95.
  - MUST-M13: an imported hit printed without its `imported` label fails the run.
  - Pass: at least 28 of 40 count, and at least v1's count plus 4 (spec 8.2 M6's 0.70 and current + 0.10, counted, not rounded); validity at least 0.95; the dated subset at least 0.70 over its own N.

### M5: resume on one device

- **Cuts** (`m5.py cuts`). Each dev session whose first and last events are 30 minutes or more apart (26 of the 30) is cut at the event with index `h(f'm5-cut:{SEED}:{session}') % n` among its n events at least 30 minutes after its first.
- **Homes** (`m5.py run`). Per session, a fresh home with that session's events up to and including the cut. The curated tier is curated as the dev home is; the none tier is a copy taken before, with `curate = false`. Each tier's `oboete inject` in the session's checkout (SessionStart's text for a new session) is kept.
- **Labels.** The labeller reads the session up to the cut, its prompts and replies without tool calls, gated, each cut at 2,000 characters, the last 80 of them, and writes the open items at the cut (work asked for or started and not finished), the next step, and the items finished or withdrawn before it (LABEL). The checker checks 20% (CHECK_LABELS); the agreement is reported.
- **Scoring** (`m5.py score`). For each labelled item the graders say whether a tier's text shows it as still open (SHOWN). Open-item recall, open items shown as open over all open items: the curated tier at least 0.80, the none tier at least 0.50. Closed shown as open, finished or withdrawn items shown as open over all of them: the curated tier at most 10%. The next step is reported, with no line. MUST-M8's and MUST-M9's lines stay Task 8's tests.

### MUST-M11's slice

`m3.py overturned` on ov-B1 with milestone 3's dev overturn pairs. Per pair, the earlier decision's labelled words are the query, through `oboete search --all --limit 10` and MCP `search` with `all=true` (`common.Mcp`), then both with history on. Counted as spec 8.4 reads MUST-M11 (A108): over the pairs whose earlier and later claims both exist and are linked, an earlier decision counts as ranked current when it shows in the current rank without its later decision directly before it, or, when it is not delivered, when it ranks above any current hit (line 0%); history recall at 10 over the pairs whose earlier decision became a claim (at least 0.80). The other pairs are reported apart, as curation's miss.

### Per-kind labels (MUST-M21)

`m3.py kinds` draws up to 15 of the dev home's claims per kind (spec 3.2's seven), in the order of `h(f'kinds:{SEED}:{uid}')`, about 100 in all, and writes them in the label format: uid, kind, text and quotes. The graders say whether the quotes bear the claim out and whether it is of its kind (KIND). Precision per kind is reported.

### Guard

A script given a held-out session or pool exits unless `--decide <id>` equals `curator` in `~/.oboete/eval/deciding.json`, which milestone 3's deciding run writes (A88): no file, or another id, is a refusal.

### Changed on 2026-10-02, before the dev runs' curation

- **The binary.** Claude Code 2.1.287 loads a built-in plugin, `cc-plugin-plugin-authoring`, that the curator's isolation check refuses, so on main the claude entry curates nothing (#323). The dev home is replayed and its keys mapped with a release build of main (0d5de61, SHA-256 `ce6c9f28c329b8857f45aa1b6ac38a3f0e6d005bcb4a7b5d8b92e14aa861fc53`); it is curated, and M6's `b` arm, M5, MUST-M11's slice and the per-kind labels run, with a release build of main plus #323 (ceda2d3, SHA-256 `ecb94d91a7beea4efa07d35e1a50d4a8f16ab1533aba96fc7e44fcd313636070`), which changes only the curator's settings. The deciding runs use a release build of main once #323 is merged.
- **One question replaced.** q05's key named a task-notification prompt among its records. Capture stores that prompt as a subagent's envelope, which no key maps to (m6.py's `mapped_record`, as m3.py's `map_labels`), so the key could not be mapped. As for a key the checker rejects, the drafter replaced the question, with another from the same session, before any run.
- **The panel's OpenAI judge and one grader.** Groq's free tier, which served `gpt-oss-120b`, is spent each day by the owner's own curation (its chain puts groq first): 6 of the first 40 key calls got 429 on tokens per day. The owner asked to use the codex subscription instead (2026-10-02). `gpt-6-sol` takes `gpt-oss-120b`'s place on the panel, in the same position, so every draw names the same place, and passed calibration first (run 4 of calib-50, docs/milestone-1.md: κ 0.84); `gpt-6-astra`, calibrated in run 3, takes its grader seat. The graders are `gpt-6-astra`, `deepseek-v4-pro` and `glm-5.3`: still three makers, none the answerer's. The keys were written again with this panel before any run: all 40, none failed or rejected, and the checker agreed on 7 of 7.

### Prompts

The harnesses' prompts are these texts, as Python format strings (`{{` is a brace), byte for byte; `test_m6.py`, `test_m5.py` and `test_m3.py` compare them with this file. Every text filled in has passed `oboete gate`.

KEY:

```text
You write the answer key for a question about a developer's earlier coding session.

Question (asked on {asked_at}):
<<<
{question}
>>>

Records from that session, numbered:
{records}

Answer the question from these records only, as briefly as it allows, in the question's language, and name the records that state the answer.
Answer with JSON only: {{"answer": "<answer>", "records": [<record number>, ...]}}, or {{"answer": null, "records": []}} if these records do not answer it.
```

CHECK:

```text
Here are a question about a developer's earlier coding session, records from that session, and an answer key written from them.

Question (asked on {asked_at}):
<<<
{question}
>>>

Records, numbered:
{records}

Answer key:
<<<
{key}
>>>

Does the answer key answer the question correctly from these records?
Answer with JSON only: {{"agree": true}} or {{"agree": false, "why": "<one sentence>"}}.
```

QUERY:

```text
Today is {asked_at}. A developer asks a coding agent about earlier work:
<<<
{question}
>>>

Write the search that finds the answer in the developer's memory of earlier sessions: a short query in the words those sessions would use, and the date range the question names, if it names one.
Answer with JSON only: {{"query": "<query>", "since": "<YYYY-MM-DD>" or null, "until": "<YYYY-MM-DD>" or null}}.
```

ANSWER:

```text
Today is {asked_at}. A developer asks:
<<<
{question}
>>>

Search results from the developer's memory of earlier sessions, each with its id:
{results}

Answer from these results only, in the question's language, and cite the ids of the results that state the answer.
Answer with JSON only: {{"answer": "<answer>", "cites": ["<id>", ...]}}, or {{"answer": null, "cites": []}} if they do not answer it.
```

CORRECT:

```text
Question:
<<<
{question}
>>>

Reference answer:
<<<
{key}
>>>

Candidate answer:
<<<
{answer}
>>>

Does the candidate give the reference answer without contradicting it? More detail is fine; another answer, or none, is not.
Answer with JSON only: {{"correct": true}} or {{"correct": false}}.
```

HOLDS:

```text
Question:
<<<
{question}
>>>

Reference answer:
<<<
{key}
>>>

Passage:
<<<
{span}
>>>

Does this passage by itself state the reference answer to the question?
Answer with JSON only: {{"holds": true}} or {{"holds": false}}.
```

LABEL:

```text
Here is a developer's coding session up to a moment, its turns numbered:
{turns}

At the end of these turns, list:
- open: the work the developer asked for or the agent started that is not finished;
- next: the step the session would take next;
- closed: the work finished, dropped or withdrawn before the end.
Write each item as one short sentence in the session's language.
Answer with JSON only: {{"open": ["<item>", ...], "next": "<step>", "closed": ["<item>", ...]}}.
```

CHECK_LABELS:

```text
Here is a developer's coding session up to a moment, its turns numbered, and labels written for that moment:
{turns}

Labels:
{labels}

Are the labels right: every open item still open at the end, every closed item finished or withdrawn before it, and no open item left out?
Answer with JSON only: {{"agree": true}} or {{"agree": false, "why": "<one sentence>"}}.
```

SHOWN:

```text
A new coding session starts with this context:
<<<
{context}
>>>

An item of work:
<<<
{item}
>>>

Does the context show this item as still open, not finished? An item the context does not mention is not shown.
Answer with JSON only: {{"shown_open": true}} or {{"shown_open": false}}.
```

KIND:

```text
A memory system wrote this from a developer's coding session, as a {kind} ({meaning}):
<<<
{text}
>>>

The quotes from the session it rests on:
{quotes}

Do the quotes bear it out, and is it a {kind}?
Answer with JSON only: {{"borne_out": true or false, "kind_right": true or false}}.
```

KIND's `{meaning}` per kind: decision, "a choice the developer made"; preference, "how the developer wants work done, beyond one task"; lesson, "what to do or avoid, learned from a failure"; fix, "how a problem was fixed: its symptom, cause and fix"; open item, "work still to do"; repo fact, "a fact about the repository or its tools"; change, "what was changed".
