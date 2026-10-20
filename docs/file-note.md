# The note on a file: claude-mem's file context on oboete's cards

Unit X5 of the claude-mem parity plan ([claude-mem-parity.md](claude-mem-parity.md) row 8), built
before the switch (spec decision 39). When the agent opens a file that past cards name, it is
told, before the read, which cards those are, so it can fetch one instead of working the file out
again. Since 2026-10-17 the claude-mem history oboete imported joins the cards (F10 to F15).

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

F1. **Where.** Claude Code: `PreToolUse` with claude-mem's matcher (`setup::CLAUDE_READS`: `Read`,
and since 13.35 the memory lookups of docs/memory-lookup.md too), which `oboete setup` adds beside
the user's own hooks. It is a read hook: it records nothing (the read's `PostToolUse` does).
Grok: its `PreToolUse`, already wired for context, when the tool is its file read, `read_file`,
whose path is `target_file` (the probe in the `oboete-dogfood` user, Grok 1.0.40, 2026-10-11: the
hook's `additionalContext` reached the model with the tool's result). The note comes after what
else that call shows (the session's first packet, the prompt's picks), whole, while the answer
stays within the 9,500 UTF-16 units Claude Code's does: Grok clips a context at 10,000 characters.
What else that call shows is cut to fit them first, each block at a line, as Cursor's start is
(CodeRabbit on #456).
Codex: its `PreToolUse`, with claude-mem's matcher for its shell and MCP read tools
(`^Bash$|^mcp__.+__(read|view|cat)(_file|_files)?$`), for the shell commands and MCP tools
claude-mem's adapter takes (the probe, Codex 0.160.0, same day: the context reached the model as a
developer message). The command splits as a POSIX shell splits it, where claude-mem's adapter
differs in two ways: a redirection and its target stay out of a command's words without ending
it (`cat a > out b` reads `a` and `b`), and a quoted operator is a word (`echo '|' cat a` reads
nothing); a newline ends a command, and a comment and a here-document's body name nothing. Like
Claude Code's, it records nothing. Codex runs a hook only once its hash
is trusted in `config.toml`; `oboete setup` writes that row for this group as for its others.

F2. **Which files.** As claude-mem: the tool's paths, at most 10, each a regular file of at least
1,500 bytes, resolved against the payload's `cwd`.

F3. **Which cards.** The checkout's repository's current cards (not replaced, K3; not forgotten,
once milestone 5's forget is in) whose `files_read` or `files_modified` name the file. The cards
consumer keeps a table `card_files(device, op_seq, n, name, path, modified)`, indexed by `name`, the path's
last part (`\\` read as `/`): the lookup takes the rows of the file's name whose path is the
file's absolute path, its path relative to `cwd`, or its path relative to the checkout's top level
(cards name files as the window's lines did, C3), and the repository and currency from the cards
themselves. The 40 newest, together with imported history (F13, F14).

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

## Imported history (parity row 8, 2026-10-17)

claude-mem's note reads all of its own observations, so after the switch the owner's claude-mem
showed a file's history from before it, where oboete's note showed only the cards made since.
`oboete import claude-mem` brought those observations over without the files they name
(docs/claude-mem-import.md I3): on this PC 147,956 imported observations name a file (64% of
them), most of them by a path relative to the project's top (73% hold one; 29% hold an absolute
path, and an observation can hold both).

F10. **What the import brings.** For each observation it reads `files_read` and `files_modified`,
JSON arrays of strings, as claude-mem's lookup reads them: anything that is not an array of
strings counts as none, an item that is no string or is empty after trimming is left out. An
observation that names a file gets, after its document's op, a files op: an op of the same
`import` kind whose body is `{uid, source, source_id, repo, session, files_read, files_modified}`,
the document's labels and its two lists, with no title, body, kind or time. Each path passes the
gate as the title and the body do (`redact::outbound_with`), and paths are kept in order, read
first, while the op as stored stays within 32 KiB (half the op cap, each path counted as JSON
writes it, escapes included), the rest left out and counted (on this PC the most an observation
names is 6,347 bytes, so none is cut); an observation none of whose paths fits gets no files op,
so a files op always names a file and its rows show which op is a uid's newest (F12). A files op
is no document: the `imported` consumer passes over it (it says so in code, not only by failing
to read one), so search, `get`, the timeline and the per-prompt block are as before. An older binary passes over it too, so the store's format
does not change (docs/store-format.md, "When it moves"): the rollback binary opens and reads a
store that holds files ops. A forget names a files op by its uid, as it names the document, and
the rescan's masks reach its paths (they are not among `OP_NAMES`).

F11. **Every import brings the files an imported observation lacks.** `oboete import claude-mem
<db>` appends, beside each document it adds, that document's files op, and also the files op of
each observation it imported before that has none under its document's repository: a store
imported before this version, or an import that stopped between a document and its files op,
gets them at its next run, and a document claude-mem moved to another project since (I3) gets
one for the repository it is filed under now, for the exclusion and sync rules that read an op's
labels. `oboete import claude-mem <db> --files` does only the second: no new document, so what
claude-mem recorded after the last import stays out (claude-mem-parity.md, the first open check),
and a document claude-mem moved since stays where it is filed, which is where its files op goes
(the plain import moves it and files its files again).
Which uids hold a files op, under which repository, is read from the ops, as `import_repos` reads
the documents'. No files op is put for a forgotten or denied uid (the append asks again, so the
import's count is of what it stored); a forget registered while
an import runs refuses the batch it falls in, as it refuses a batch of documents, and the import
stops with what it appended before, which a second run finishes. Both run under the import lock,
in batches as the documents are.

F12. **The index.** A consumer of its own, `imported_files`, keeps
`imported_files(rowid, op_device, op_seq, uid, name, path, modified, named)` with indexes on
`(name, path)` and on `uid`: one row per path of each list, as `card_files` keeps a card's (`\`
read as `/`, `name` the last part), each with the number of paths its op names in all (`named`,
F6), so the lookup counts nothing. A uid's files are those of its newest files op, the one of the
highest rowid, as a document's text is its newest row's. It has its own checkpoint, so a rollback's
older binary, which passes over files ops, cannot move it past one it never read; on a store
from before, its first pass reads the ops from the start. A rewind deletes the rows of the ops it
takes back, a rebuild makes them again from raw.db, and a forget's purge rewrites a files op as it
rewrites the document's, so the rebuild after it has no rows of the uid (forget's
`knowledge_holding` names the table). Two files ops of one uid (two devices, or a moved document)
both give rows; the lookup reads the newest op's alone.

F13. **Which imported observations.** One joins a file's note when its newest row maps to the
checkout's repository by its session, as imported history maps for the per-prompt block (#506,
docs/prompt-context.md, "As built": `session_start`), and its files rows name the file by one of
F3's three paths. Not a forgotten one, and never a summary or a prompt (they have no files op).
An observation of a session oboete never recorded maps nowhere and stays search-only, as for the
per-prompt block, where claude-mem maps by the project's name: on this PC 66% of the imported
observations that name a file map (all of this repository's 47,004, half of the other projects').
Whether the rest should map by name, as claude-mem's does, is the owner's to decide (parity row 8).
Until the consumers have made `imported_files`, `imported` and `session_start` (a store from
before), the note has no imported rows and is the cards' alone; while a first pass or a rebuild
is still going, it has the rows indexed so far, as `card_files` has the cards'.

F14. **One pool.** The note takes the 40 newest of F3's cards and F13's observations together, by
time, as claude-mem takes its 40 from one table; F4 to F7 then hold for both alike. F4's newest
is the newest of the 40. F5 keeps every ID considered, an observation by its uid
(`claude-mem:<source>:o<id>`, which `get` takes). F6 keys an observation by its session with no
agent; its modified flag and its number of files come from its files rows. F7 shows its uid, its
time, its kind's icon and its title, gated as the search gates an imported title, cut at 160
characters: claude-mem's `refactor` and the types oboete does not keep came in as `discovery`
(I3), so they show 🔵, and an imported `preference` shows ❓.

F15. **F8, checked.** A read in a checkout nothing is recorded from gets no note and writes no
hook state: the reason `capture::not_recorded_why` gives, a repository, folder or pattern never
recorded (milestone 5), or the checkout's `.oboete.toml` with `capture = false` or one that does
not read. Until now F8 held only because such a checkout makes no cards; an import does not pass
capture exclusion, so its history would come back at a read, and so would the cards it made
before it was excluded.

F9 for the second kind: the lookup is one query by `(name, path)`. It is timed before the files
ops reach this PC's store, on a store of the same size (147,956 observations' files, a name
thousands of them share, ten files a read), and again on this PC's store after `--files`.
Before (2026-10-20, a synthetic store of 250,000 imported rows, 147,500 with files and 421,271
path rows, ten names shared by thousands): a read of ten such files took 144.5 ms at the median,
162 ms at p95 and 282 ms at most, about 14.5 ms a file for the 2,300 rows that share its path.
The cost grows with the rows that share the file's path in every project, about 5 µs each, and
the lookup reads them all before it keeps the 40: on this PC the most observations one path has
is 3,793, and 28 paths have 1,000 or more, so a read of the hottest file would take some 20 to
30 ms, within the times of the other read hooks (session start at 135 ms at the median, the
per-prompt block at 74 to 147 ms, claude-mem-parity.md X2) and under claude-mem's. If the
measurement after `--files` puts a read of a hot file past them, the index keeps each row's time
and whether its op is its uid's newest, and the lookup walks the rows newest first, stopping at
the 40th.

## Slices (test first, one at a time)

1. **The index and the lookup**: `card_files` in the cards consumer (with its backfill, as
   `cards_fts` has), and `cards::by_file`, which gives the rows and the text (F3, F4, F6, F7).
2. **Claude Code**: the hook (F1, F2, F5, F8), `oboete setup`'s matcher, and the measurement (F9).
3. **Grok, then Codex**, each after its probe in the dogfood user (G25; both probes passed on
   2026-10-11).
4. **Imported history** (F10 to F15), one PR: the files ops and `--files`, the consumer, the
   lookup in one pool, F8's check; then, once installed, `--files` on this PC's store and F9's
   measurement again.

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
12. Grok's `read_file` gets the note after the session's first packet, once a session; a note with
    no room left waits, unclaimed, for a later read; another tool gets none.
13. Codex's shell reads get the note (`cat`, `head -n 5` with two files, a reader after a pipe and
    `--`, a quoted path, a reader by its full path, a command given as words), and so do an MCP
    read tool's `path` and `paths`; `ls`, an option's value, a redirection's target, a command
    with an open quote, an MCP write tool and another tool get none. The files after a
    redirection are read; a quoted operator splits nothing; an operator needs no space around it;
    a newline splits as `;` does; a comment names no command or file, and nor does a
    here-document's body (`<<` and `<<-`, to its delimiter's line) or a here-string.
    The hook takes for an MCP read what Codex's matcher sends it.
14. `oboete setup` adds Codex's `PreToolUse` group with that matcher after the user's own, and the
    row that trusts it.
15. An import of observations that name files appends a files op after each such document and
    none for one that names none; the op holds the gated paths and no title or body; paths past
    the 32 KiB of the op as stored are cut and counted, also paths of quotes, backslashes and
    control characters, which JSON writes longer, and many short ones; a malformed list is none;
    an observation whose only path does not fit gets no files op.
16. `--files` on a store imported before appends one files op for each imported observation that
    names a file, none for a forgotten or denied uid, and no document; a second run, and a plain
    import after it, append no files op again. A plain import that stopped after a document and
    before its files op appends that files op at its next run; a document moved to another
    project gets a files op under its new repository, and `--files` gives one moved before it had
    any a files op where it is filed. The import's line counts the files ops it stored and, under
    `--files`, as seen only the observations imported before.
17. A files op reads as no document: search, `get`, the timeline and the per-prompt block give
    what they gave without it.
18. `imported_files`: a row per path of each list, each with its op's number of paths; a rewind
    drops the rows of the ops it takes back; a rebuild makes the same rows; a forget's purge and
    the rebuild leave none of the uid, and `knowledge_holding` names it until then. Two files ops
    of one uid with different lists: the newest op's paths, flag and number alone count.
19. A read of a file that an imported observation of a mapped session names, by its absolute path,
    its path relative to `cwd` or to the checkout's top, gives the note with its uid, time, icon
    and title; an observation of an unmapped session, of another repository's session, or a
    forgotten one gives none.
20. Cards and imported observations share the 40 and the 15: one row a session, ranked as F6, and
    F4 against the newest of both.
21. A session that was shown a file's note with an imported row gets it again only when a row
    that was not considered names the file.
22. A read in a checkout excluded by repository, by folder or by pattern, or with
    `capture = false` or an unreadable `.oboete.toml`, gets no note and writes no hook state, also
    where cards and imported history name the file from before the exclusion.
23. A store whose consumers have not made `imported_files`, `imported` or `session_start` gives
    the cards' note as before.
24. A forget registered after an import's check of forgotten uids: the batch it falls in is
    refused, nothing of the uid is stored, and a second run appends the rest.
