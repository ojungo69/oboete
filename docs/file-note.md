# The note on a file: claude-mem's file context on oboete's cards

Unit X5 of the claude-mem parity plan ([claude-mem-parity.md](claude-mem-parity.md) row 8), built
before the switch (spec decision 39). When the agent opens a file that past cards name, it is
told, before the read, which cards those are, so it can fetch one instead of working the file out
again.

## What claude-mem does

Read in the installed plugin, 13.29.0 (`hooks/hooks.json`, and the bundled handler
`scripts/worker-service.cjs` built from `src/cli/handlers/file-context.ts`,
`file-context-dedupe.ts` and `src/cli/adapters/codex-file-context.ts`):

- **When.** Claude Code's `PreToolUse` with the matcher `Read` runs `hook claude-code
  file-context`. A subagent's read (`agent_id` in the payload) and a project excluded from
  recording get nothing. The paths are the tool input's `filePaths` (at most 10) or `file_path`.
- **Which files.** Each path, resolved against the payload's `cwd`, must be a regular file of at
  least 1,500 bytes. A missing file gets nothing.
- **Which observations.** The worker's `/api/observations/by-file` with the absolute path and the
  path relative to `cwd`, at most 40. None: nothing. When the file was modified at or after the
  newest of them, nothing: they may describe an older file.
- **Once a session.** A small table in its own database, keyed by session and file, keeps the time
  of the newest observation shown; the note comes again in that session only when a newer one
  names the file. Rows older than 7 days are dropped.
- **Which rows.** One observation per session (the first the worker returned), ranked by how
  specific it is to the file: +2 when the file is among its modified files, +2 when it names at
  most 3 files in all, +1 when at most 8; the 15 best, in that order before grouping.
- **The text**, one block per file, blocks joined by `---`:

  ```text
  Current: <the local time>
  This file has prior observations — supplementary context follows. The Read result below is the full requested section.
  - **Need details on a past observation?** get_observations([IDs]) — ~300 tokens each.
  - **Need a structural map first?** smart_outline("<path>") — line numbers only, cheaper than re-reading.
  ### <day>
  <id> <time> <icon> <title>
  ```

  Days in the order of their first row, rows oldest first within a day; the title on one line, at
  most 160 characters, `Untitled` when empty; the icons are this note's own (`⚖️` decision, `🔴`
  bugfix, `🟣` feature, `🔄` refactor, `🔵` discovery, `✅` change, `❓` any other type), not the
  session start's.
- **The answer** is `hookSpecificOutput` with `hookEventName: "PreToolUse"`, the text as
  `additionalContext`, and `permissionDecision: "allow"`.
- **Codex.** The adapter takes the paths of a shell command whose first word is `cat`, `head`,
  `tail`, `less`, `more`, `bat`, `view`, `nl` or `tac` (the values of `head`'s and `tail`'s `-n`
  and `-c` are not paths), and of an MCP tool named `mcp__<server>__read`, `__view` or `__cat`
  (with `_file` or `_files`) from its `path` or `paths`; at most 10.

## What oboete does

F1. **Where.** Claude Code: `PreToolUse` with the matcher `Read`, which `oboete setup` adds beside
the user's own hooks. It is a read hook: it records nothing (the read's `PostToolUse` does).
Grok: its `PreToolUse`, already wired for context, when the tool is its file read. Codex: its
`PreToolUse`, for the shell commands and MCP tools claude-mem's adapter takes, once a probe in the
`oboete-dogfood` user shows that the installed Codex shows a `PreToolUse` hook's context to the
model; until then Codex has no note and the doctor says so (Claude; overrulable).

F2. **Which files.** As claude-mem: the tool's paths, at most 10, each a regular file of at least
1,500 bytes, resolved against the payload's `cwd`.

F3. **Which cards.** The checkout's repository's current cards (not replaced, K3; not forgotten,
once milestone 5's forget is in) whose `files_read` or `files_modified` name the file. The cards
consumer keeps a table `card_files(device, op_seq, n, name, path, modified)`, indexed by `name`, the path's
last part (`\\` read as `/`): the lookup takes the rows of the file's name whose path is the
file's absolute path, its path relative to `cwd`, or its path relative to the checkout's top level
(cards name files as the window's lines did, C3), and the repository and currency from the cards
themselves. The 40 newest.

F4. **Not when the file changed since.** When the file's modification time is at or after the
newest such card's time, there is no note, as in claude-mem.

F5. **Once a session.** The hook keeps, for a file, the IDs of every card it considered (the
current cards naming the file, the 40 newest of F3, not only the rows F6 shows) in the session's
hook state (`hookstate`, under the agent and session, where the hooks already keep flags such as
`injected` and `compacted`); the note comes again in that session only when a current card naming
the file is not among them: a newer card, or a recuration's replacement, which keeps its window's
time (docs/cards.md K2, K3), so a time alone would hide it (Codex on #384). Keeping only the rows
shown would bring the note back at every read of a file with more cards than rows (Codex on #384). The worker's pruning of
the hook state (`hookstate::KEEP`) removes it.

F6. **Which rows.** claude-mem's: one card per session, the newest; ranked +2 for a modified file,
+2 for at most 3 files named in all, +1 for at most 8; the 15 best; then by day as above. The
session (with its agent) and whether the card modified the file are taken as stored, before the
gate masks the card's fields: a masked session or path would merge sessions or lose the +2
(Codex on #387).

F7. **The text.** claude-mem's, with oboete's tools and IDs: `get(ID)` in place of
`get_observations([IDs])`, and no `smart_outline` line (oboete has no code-structure tools, parity
row 24). IDs as the session start's block writes them (`<op seq>.<n>`, with the device for another
device's card, docs/cards.md S3). Each title is gated as that block gates a row (K6), then cut at
160 characters. The icons are claude-mem's for this note. The note is fenced as data and
attributed, as every injection is (spec 4: the session start's fence, each closing tag in it
escaped as that fence escapes it, docs/cards.md S5): a title may summarize hostile file or tool
output (Codex on #384). `cards::by_file` is one of K7's ways in: it reads the cards through K3,
K4 and K6, never the table itself.

F8. **Never.** No permission decision: `permissionDecision: "allow"` would let a read through
without the owner's own permission rules, which is not oboete's to change. No note for a
subagent's read or in a checkout excluded from recording (milestone 5's capture exclusion). No
call off the machine and no model: the hook reads the knowledge store the worker keeps and the
file's metadata, the second part of 4.1's rule. A failure is no note and a line on stderr; the
read goes on.

F9. **Cost.** Each read spawns one more short hook in Claude Code. It is a read hook in the sense
of spec 8.2: its own line, set from its first measurement (the p95 of the whole spawned hook, its
hook-state write included) on the replay fixture's Claude Code read sample, and kept for every
later run.

## Slices (test first, one at a time)

1. **The index and the lookup**: `card_files` in the cards consumer (with its backfill, as
   `cards_fts` has), and `cards::by_file`, which gives the rows and the text (F3, F4, F6, F7).
2. **Claude Code**: the hook (F1, F2, F5, F8), `oboete setup`'s matcher, and the measurement (F9).
3. **Grok, then Codex**, each after its probe in the dogfood user.

## Tests

1. A read of a file of 1,500 bytes or more that current cards name gives the note: the three
   lines, then the days in order and the rows with ID, time, icon and title.
2. A file under 1,500 bytes, a directory, a missing file, a file no card names: no note.
3. A file modified at or after the newest card naming it: no note.
4. The same session reading it again: no note, also for a file with more than 15 cards or two
   cards of one session; once a newer card names it, or a recuration replaces a card (with the
   same time): the note again.
5. A card of another repository, a replaced card, a forgotten card: never in the note.
6. A card naming the file by its path relative to the checkout's top level, or by its absolute
   path, is found; one naming `foo+bar.rs` is not found for `bar.rs`.
7. At most 15 rows, one per session, ranked as F6.
8. A title is gated as the session start's row is, and cut at 160 characters; the note is inside
   the session start's fence, and a title holding the fence's closing tag is escaped.
9. A subagent's read, and a read in an excluded checkout: no note.
10. The answer has no `permissionDecision`, and the hook writes no raw record.
11. `oboete setup` adds the `PreToolUse` `Read` matcher for Claude Code and keeps the user's own
    `PreToolUse` hooks.
