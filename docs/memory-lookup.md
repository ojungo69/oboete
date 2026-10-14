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
  (docs/mem-search.md), on the words side alone: the hook embeds nothing and calls no provider
  (decision 38). It comes after claude-mem's sentence (with oboete's name) inside the memory fence,
  through the egress gate, the whole within 9,500 UTF-16 units and cut at a line, as the file
  note's room is. It holds no continuation (none would outlive the hook: claude-mem's hook and
  tool share one worker's store) and no muted claim, since it is injected (spec 6.1). When it
  answers, the file note waits for the next read; the native lookup goes on either way.
- **None**: for any other call; with `[inject] memory_lookup = false` (on by default, as claude-mem's
  `CLAUDE_MEM_MEMORY_SEARCH_HOOK_ENABLED`), on the detailed settings page; where nothing is
  recorded; before the stores are made; when the search fails. A subagent's lookup gets it too, as
  claude-mem's handler runs before its subagent check.

## Differences from claude-mem, and why

| claude-mem | oboete | Why |
|---|---|---|
| The worker searches, through HTTP, waiting at most 3 s | the hook searches the stores itself | oboete's hooks read the stores directly; no worker is waited for |
| Search with its vectors (Chroma) | the words side alone | a hook embeds nothing (decision 38) |
| `<claude-mem-retrieved-data>`, its text cut at 10,000 characters and the sentence and tags added | oboete's fence, the whole within 9,500 UTF-16 units | Grok and Claude Code clip a context at 10,000 (docs/file-note.md F1) |
| Folders of `CLAUDE_MEM_MEMORY_WATCH_ROOTS` too | not yet | claude-mem's folder watcher is parity row 44, after the switch |

## Cost

Every Bash, Grep and Glob call of Claude Code and Codex now starts the hook, as it starts
claude-mem's. A call that is no lookup returns before any store is opened, after reading at most
three small settings files for `autoMemoryDirectory`. Measured on this PC's WSL on 2026-10-14 (the
release build, 150 calls each): a Bash call that is no lookup took p50 1.8 ms, p95 2.3 ms, max
3.6 ms, as a Read of a file that is no memory did (p50 1.8 ms, p95 2.1 ms), which the hook has
always been sent.

## Tests

`memory_lookup::tests::a_lookup_is_told_as_claude_mem_tells_one` (claude-mem's cases: native
searches, the memory tool, reads of a memory folder by each tool and shell reader, grep's and rg's
options, paths with `~/` and `..`, and the calls that are none),
`a_command_splits_into_its_words` (a glob too), `an_mcp_reads_paths_are_its_candidates`; `hook::tests::a_native_memory_lookup_gets_oboetes_search_of_it`
(the answer, a subagent's, a rule added later, the index with no continuation, a muted claim left out, a call that is none, the switch off);
`setup::tests::claude_gets_a_pre_tool_use_hook_for_its_reads_beside_the_users` (the matcher);
`settings::tests::inject_settings_check_ranges_and_save_alone` (the switch).
