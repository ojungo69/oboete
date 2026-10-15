# Switch rehearsal, 2026-10-15

The rehearsal of spec 7.5 in the isolated `oboete-dogfood` user, on copies of the owner's v1 store and claude-mem database: the last item of decision 45's list before the switch. Times are wall clock on this PC (WSL2). The copies were deleted afterwards.

## Setup

- Agent CLIs at the owner's versions: Claude Code 2.1.300, Codex 0.162.1, Grok 1.0.54.
- The v1 store and the claude-mem database copied with `sqlite3 .backup` (5.2 s and 4.3 s; both in WAL mode, their writers not blocked); `quick_check` ok. The owner's v1 config, without `[embedding]`.
- The v1 binary set up for Claude Code, Codex and Grok. Every v1 session in dogfood ran with `OBOETE_NO_SPAWN=1`, as v1's summary chain ends with the `codex` and `claude` CLIs on PATH.
- v1 doctor: 308 sessions, 173,517 events, 7,812 observations, 1,204 summaries.

## Migration and imports

Two passes, the second with the fixes below (ranges cover both):

- `migrate`: 147-174 s, peak RSS 56-61 MB: 173,517 events to 173,517 records, 295 repositories, 9,738 documents. It prints `[summary] curate is not set` and the defaults of `[inject]`, `[capture]`, `[redaction]` and `[chain]`.
- `import transcripts` (dogfood's own): Claude Code 246 sessions and 972 events, Codex 66 and 206; 2-3 s.
- `import claude-mem`: 89-112 s, peak 141 MB: observations 223,477, summaries 14,170, prompts 13,871, empty 2,118, work state 1,103, refused 0.
- `worker --idle-ms 0`: 264-330 s, peak 80-84 MB; knowledge.db 1.3 GB, raw.db 1.24 GB.
- doctor: `quick_check` ok; imported and not curated 174,695; embeddings off; raw.db integrity ok.

Findings:

1. `import claude-mem` refused claude-mem's schema v65 (13.36.0 on; its one step recomputes `observations.title_norm_key`). Fixed in #534.
2. The rescan tombstoned 259 ranges on 232 v1 tool events, each over a `[REDACTED]` inside `scheme://user:[REDACTED]@host`, and doctor asked for `forget --continue`: `uri-userinfo-password` has no entropy gate, and the bundled rules' loop lacked the user rules' skip of capture's own mask. Fixed in #535; the second pass had no rescan tombstone.

## Search without model calls

A114 on a copy of the evaluation import (the new binary rewrites a copied home's raw.db and knowledge.db when it opens them, so it never runs on the frozen homes themselves): B's full-text results for the 312 dev questions, each question's own prompt left out, against the frozen `e0-trigram` run.

- overlap@10: mean 0.35, median 0.30, 68 questions with none.
- nDCG@10 on the 306 dev questions with an answer: strict (an unjudged result counts 0) e0 0.436 and B 0.371, with 34% of B's top 10 unjudged; judged results only, e0 0.436 and B 0.504; recall@10 0.149 and 0.190; hit@10 0.820 and 0.863.
- Known v1 uids of observations, summaries and prompts all fetched by `get`; a search for an observation's title puts it first.
- MCP: `get`, `mem_search`, `save_memory`, `search`, `timeline`, `work_state_read` and `work_state_write`. `mem_search` mixes claude-mem and v1 documents; a note saved with `save_memory` is found first by `search` and by the CLI.

## Hook switch and live sessions

- `setup` for the three agents: Claude Code 8 hooks, Codex 7 with trust 7/7, Grok 8, MCP registered in each; the resident worker serves the page on 127.0.0.1.
- `migrate` again: nothing new.
- Per-prompt injection is off by default. A user who had claude-mem's per-prompt semantic injection on turns `[inject] per_prompt` on at the switch.
- Codex: the SessionStart message holds the work-state rule, the memory-use block (#531), open work state and the manifest; a read of a file under `~/.codex/memories/` got the memory-lookup answer (#530); `mem_search` and `save_memory` worked. The resumed second prompt got no per-prompt block: curation was off, and imported documents are never injected. Subagent compaction (#532) was not reachable in a live session.
- Claude Code and Grok were not run in dogfood (its login and account limits).

## Settings page

- Simple view in English and Japanese: every block drawn. Finding 3: the word `null` under the lead (an absent part appended to the page). Fixed in #536.
- Detailed view: the per-prompt wait (100-3,000 ms), the last prompt's side, the memory-lookup and memory-use switches, corrections. No claim to mute or forget, as curation was off.

## Rollback drill and finish

- Finding 4: `cp <v1> oboete` failed with `Text file busy` while the resident worker and viewer ran that path. A rename works: `install -m 755 <old> .oboete.swap && mv -f .oboete.swap oboete`. Both resident processes left within a minute of the swap (spec 7.5, Rollback).
- A v1 Codex session wrote 5 events. Forward again by rename: doctor showed 5 v1 events not migrated, and `migrate` imported exactly 5.
- `migrate --finish` (yes): one more pass, then oboete.db and its `-shm` and `-wal` deleted; doctor shows no v1 lines.
- `forget` of one document on this store: 284 s, peak 77 MB, mostly the rebuild of knowledge.db (1.3 GB). A session starting meanwhile gets part of its memory.

## Transcript import at the owner's scale

A preview, which writes nothing, against a copy of the migrated raw.db: Claude Code 6,278 files, 6,228 sessions, 369,985 events, 1.43 GB; Codex 2,373 files, 2,352 sessions, 137,167 events, 1.38 GB. The preview took 695 s (peak 202 MB).

## Final check

On the tree of main 342bec4, with #535 merged (a release built with `--features local-embed` before the merge, from the same tree): a fresh copy of the owner's v1 store, 174,228 events by then, migrated in 144 s (295 repositories, 9,793 documents); claude-mem imported in 54 s with the same counts; the worker done in 247 s (peak 85 MB). doctor: `quick_check` and integrity ok, embeddings off, no line asking for `forget --continue`, and no range tombstone. A live Codex session got the SessionStart work-state block, the memory-lookup answer after reading its memory file, and `mem_search`, `save_memory` and `search`, which found the saved note first. The settings page's simple view, in Japanese and in English, shows no `null`.
