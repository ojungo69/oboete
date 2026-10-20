# The memory lookup bridge: claude-mem's answer to an agent's own memory lookups

claude-mem 13.35 answers an agent that looks up its own memory, its notes folder or a native
memory search, with its own search of the same words, beside the native result. Decision 45 puts
it before the switch. The behaviour of record is claude-mem's
`src/cli/handlers/memory-search.ts` at v13.39.0 (Apache-2.0; NOTICE); nothing of its code is
taken. The code is `src/memory_lookup.rs`, run from the prompt hook's `PreToolUse` before the file
note (docs/file-note.md).

## What it does

- **When.** Claude Code's and Codex's `PreToolUse`, with claude-mem's matchers
  (`setup::CLAUDE_READS`, `setup::CODEX_READS`: their reads, and Bash, Grep, Glob, `memory`,
  `memory_search` and an MCP server's `memory_search`). Grok gets nothing, as from claude-mem.
- **What is a lookup** (claude-mem's `memorySearchLookup`): `memory_search` or an MCP server's
  `memory_search` with a query; the `memory` tool's `view` or `search`; a Read, Grep, Glob or MCP
  read (`read`, `view`, `cat`, `_file`, `_files`) of a memory folder; a Bash command with a reader
  (`rg`, `grep`, `cat`, `head`, `tail`, `sed`) naming one. The memory folders: Claude Code's
  `projects/*/memory` under `CLAUDE_CONFIG_DIR` or `~/.claude`, each `autoMemoryDirectory` its user
  or checkout settings name (absolute or under `~/`), and Codex's `memories` under `CODEX_HOME` or
  `~/.codex`. claude-mem's own tools, and oboete's, are no lookup. A shell word with an unquoted
  `*` or `?` is a glob and names no file, as shell-quote reads it for claude-mem; an MCP read's
  `paths` count when they are files, as claude-mem's Codex adapter takes them.
- **Its words**: the query; a grep's or rg's pattern (`-e`/`--regexp` first, none when `-f` names a
  pattern file, an option's value never); a tool's `pattern`; else the file's name, `MEMORY`
  standing for "preferences decisions corrections". At most 500 characters and 1,024 bytes.
- **The answer**: `mem_search` in auto mode, at most 8 rows, of the checkout's repository
  (docs/mem-search.md), with the meaning side too when the resident viewer makes the lookup's
  query vector within `[inject] per_prompt_wait_ms` (`POST /api/query-vector`, as the prompt asks
  it for its block, G23), else the words side alone: the hook embeds nothing itself and calls no
  provider (decision 38). Since 2026-10-17 (parity row N5). It comes after claude-mem's sentence (with oboete's name) inside the memory fence,
  through the egress gate, the whole within 9,500 UTF-16 units and cut at a line, as the file
  note's room is. It holds no continuation (none would outlive the hook: claude-mem's hook and
  tool share one worker's store) and no muted claim, since it is injected (spec 6.1). When it
  answers, the file note waits for the next read; the native lookup goes on either way.
- **None**: for any other call; with `[inject] memory_lookup = false` (on by default, as claude-mem's
  `CLAUDE_MEM_MEMORY_SEARCH_HOOK_ENABLED`), on the detailed settings page; where nothing is
  recorded; before the stores are made; when the search fails; when it has not answered 3 s after
  the hook started, the vector's wait included (`LOOKUP_WAIT`, claude-mem's `timeoutMs: 3_000` for
  its worker's search), the search left on its thread until the hook exits, and then no file note
  either, since the wait spent the hook's time. A subagent's lookup gets it too, as claude-mem's
  handler runs before its subagent check.

## Differences from claude-mem, and why

| claude-mem | oboete | Why |
|---|---|---|
| The worker searches, through HTTP, waiting at most 3 s | the hook searches the stores itself, at most 3 s from its start | oboete's hooks read the stores directly; the 3 s keep the hook within Claude Code's 5 s, as claude-mem's do |
| Search with its vectors (Chroma) | with the resident viewer's query vector within the prompt's wait, else the words side alone | a hook embeds nothing itself (decision 38); the viewer holds the model, as for the prompt's block (G23) |
| `<claude-mem-retrieved-data>`, its text cut at 10,000 characters and the sentence and tags added | oboete's fence, the whole within 9,500 UTF-16 units | Grok and Claude Code clip a context at 10,000 (docs/file-note.md F1) |
| Folders of `CLAUDE_MEM_MEMORY_WATCH_ROOTS` too | not yet | claude-mem's folder watcher is parity row 44, after the switch |

## Cost

Every Bash, Grep and Glob call of Claude Code and Codex now starts the hook, as it starts
claude-mem's. A call that is no lookup returns before any store is opened, after reading at most
three small settings files for `autoMemoryDirectory`. Measured on this PC's WSL on 2026-10-14 (the
release build, 150 calls each): a Bash call that is no lookup took p50 1.8 ms, p95 2.3 ms, max
3.6 ms, as a Read of a file that is no memory did (p50 1.8 ms, p95 2.1 ms), which the hook has
always been sent.

## Speed (2026-10-20)

Claude Code's record of the hook on this PC from 2026-10-15 to 2026-10-20: 59 lookups, the median
5.0 s, 51 over 3 s, 39 ended at its 5 s with nothing shown. claude-mem's bridge took a median of
247 ms over the same days. One lookup of a memory file's name, the stores' pages dropped from the
page cache first, took 9.3 s, of which:

- 4.9 s the imported leg's full text: the trigrams of a few words match nearly every imported
  document (235,789 of 263,208), and its join read each match's row (428 MB) for its filters and
  order, before the 40 best were taken. The ranking alone took 0.9 s.
- 2.1 s the rescoring of the vector side's candidates (`knn_scored`: 400 a leg, 2,092 in all), each
  fp32 vector a few random pages of a 1.1 GB table read one after another.
- 1.5 s the records' leg (ranking 0.8 s, their text for snippets 0.7 s), whose rows come after
  the other legs' 100 documents and so never reached the 8 shown.
- The rest the bit index's scans and the legs' hits. A lookup whose index has a title of its words
  also reads the context of each such anchor, a timeline: there the repository's imported
  documents were read whole with their text for each side (5.4 s cold, 0.75 s warm around one
  anchor), and its prompts by a scan of every record (0.85 s cold). The imported leg's join had
  read the same documents, so its cost had hidden this one.

What changed (each gives the same answers: 16 lookups in another home with this PC's store, 10 on
the store itself with the resident viewer's vectors, and 54 CLI searches and timelines, all byte
for byte the same as before):

- The imported consumer makes a narrow index of the columns that join reads
  (`consumer::imported::COVER`, `imported(rowid, repo, ts, kind, session, uid)`: 31 MB on this
  store), which `search::b::imported_fts` reads when it exists: a reader never makes an index, since
  a hook can be ended mid-build. Short words' pool reads the title and body of its `POOL` rows
  alone. Cold, the join took 0.6 s instead of 3.9 s on a copy of the store.
- The rescoring reads its first 8 vectors, and when they took more than 0.2 ms each (the disk, not
  the page cache) it reads the others ahead on 8 read-only connections at once, kept for the
  search's other legs (`search::b::prefetch_vectors`): 400 cold vectors took 0.15 s instead of
  0.70 s, and a warm search opens none. Nothing read ahead is used: a vector never changes under
  its `src_sha`.
- The records' leg, below the others in relevance order (`mem_search`'s, the CLI's default), is
  neither ranked nor read when the other legs have as many documents as the answer's limit, since
  none of its rows could take a place (each leg keeps to a kind filter before its depth); by date
  it is read, and its ranking still runs while a query's call to the embedder is out (D8).
- A timeline reads a repository's imported documents by a second index, `imported(repo, ts, uid)`
  (`imported_time`), each of its two names to the page's end in the side's own order, and its
  prompts by a partial index of the prompt records (`raw_docs_prompts`), both the consumers'.
- The 3 s above, so a lookup the cache cannot answer in time costs 3 s and not Claude Code's 5.

The speed-ups serve every search and timeline, `mem_search` and the viewer's included.

After, on a copy of this PC's store in another home with the resident viewer's query vectors
(2026-10-20): the lookup above took 2.66 s cold and 0.79 s warm (9.66 s and 1.52 s before); five
memory files' lookups, cold, answered in 2.50 to 2.87 s but for MEMORY.md's ("preferences
decisions corrections", words most documents hold), which passed 3 s and gave nothing, and warm in
0.76 to 1.02 s. What a cold lookup still reads: the imported ranking (0.7 s), the bit index's scan
of the imported partition (0.6 s) and the legs' hits.

## Tests

`hook::tests::a_memory_lookup_past_its_wait_gives_nothing` (a forget holding the search back: an
error within the wait, as the hook reports a failure, the native lookup going on),
`a_memory_lookups_wait_holds_the_viewers_vector_too` (a viewer slower than the wait costs no more),
`a_memory_lookup_past_its_wait_takes_no_file_note_after_it` (a forget holding the stores back costs
the hook its wait alone);
`search::b::tests::imported_full_text_reads_the_narrow_index_and_finds_the_same`,
`a_timeline_reads_a_repositorys_documents_in_time_order_and_finds_the_same`,
`by_date_a_newer_record_comes_before_imported_documents_that_fill_the_limit`,
`prefetch_reads_a_files_vectors_and_skips_memory`;
`search::b::timeline::tests::a_repositorys_prompts_are_read_by_their_index`;
`memory_lookup::tests::a_lookup_is_told_as_claude_mem_tells_one` (claude-mem's cases: native
searches, the memory tool, reads of a memory folder by each tool and shell reader, grep's and rg's
options, paths with `~/` and `..`, and the calls that are none),
`a_command_splits_into_its_words` (a glob too), `an_mcp_reads_paths_are_its_candidates`; `hook::tests::a_native_memory_lookup_gets_oboetes_search_of_it`
(the answer, a subagent's, a rule added later, the index with no continuation, a muted claim left out, a call that is none, the switch off);
`setup::tests::claude_gets_a_pre_tool_use_hook_for_its_reads_beside_the_users` (the matcher);
`settings::tests::inject_settings_check_ranges_and_save_alone` (the switch).
