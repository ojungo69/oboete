# claude-mem's history in the everyday store (N1)

Row N1 of the parity plan (docs/claude-mem-parity.md) and G54 of the 13.34.2 comparison:
`oboete import claude-mem <db>` brings the owner's claude-mem database into the store the hooks
write, its work state with it (docs/work-state.md L10), before claude-mem is removed (decisions 35
and 37). Until now it imports into evaluation homes only (`--eval-store`, docs/pr-b.md:21).

## Why the guard can go

docs/pr-b.md kept the import out of the everyday store because 160,000 rows that no repository's
injection could find would have come in before repositories mapped onto claude-mem's project
names. Since then imported documents are injected only through the session they were recorded in,
never by a project's name (spec 4.5, #506), and search finds a
repository's imported documents by name: those of the claude-mem project its key ends in, and of
that project's worktree sessions (`imported_repos`, milestone 4 Task 4). Work state needs the same
rule and nothing more.

## Rules

I1. **Where.** Without `--eval-store` the import goes into the home given, the default one
included. `--eval-store` keeps its meaning, an evaluation home: it still refuses the default home.

I2. **Which databases.** claude-mem's schema from 13.28.0 (migrations to v52) to 13.40.0 (v67).
The importer reads what it finds by table and column (`PRAGMA table_info`), not by version number:
another claude-mem draft (#4341) also numbers a migration v61. v65 (13.36.0,
`refreshUnicodeTitleNormKeys`) only recomputes `observations.title_norm_key`, the key claude-mem
finds a duplicate title by, which the import does not read; 13.36.0 to 13.39.0 change no other
table or column. v66 (13.40.0) adds cloud sync columns to `work_state_entries` (`synced_at`, the
origin device and row, `sync_rev`) and v67 lets the sync ledgers name the work state kind; the
import reads neither. A database whose `schema_versions` names a version above 67 is refused, with
nothing imported: a newer claude-mem may keep what this import would leave behind. The message
names both versions.

I3. **What comes in.** As before: observations (the title; the narrative, then each fact), session
summaries and prompts, as import ops, gated on the way in, each once (by source id). Two changes:

- a summary's `notes` comes in as its last line, `Notes: …`, as claude-mem 13.34.2 shows them
  (#403);
- a row claude-mem merged into another project (`merged_into_project`, its `ProjectMerge`) comes in
  under that project, where claude-mem reads it; one merged after it came in comes in again, with
  the same uid, and readers take a document's newest row (Codex on #431).

I4. **Work state.** Each `work_state_entries` row (v61) comes in as a work state op, oldest first:
the list, the fields, `repo` the project as an import names it (`claude-mem:<project>`), its own
time as both `clock` and `at`, and its source and id (`w<id>`), so a second run adds nothing. It
passes the checks and the gate a write passes (L3, L5); a row they refuse is counted and left out.

I5. **Reading work state.** A repository's work state is its own entries and those of the
claude-mem project its key ends in, the project's worktree keys (`claude-mem:<name>/…`) included,
as search reads its imported documents, folded together by clock: an imported list goes on under
the repository's own writes. An imported entry takes effect at its own time, also when this
device's writes reached the store before it, or at its import when its time is later (a clock ahead
of this device's), and lifts no clock of theirs, when they are read or written; it shows its own
time (`at`). The name reads imported entries only, as search's reads imported documents only: a native
entry is read by its own key alone, whatever its shape (an origin like `ssh://claude-mem:<name>/x`
makes a key shaped like a worktree project's). A checkout whose key ends in a project's name reads
that project's lists, as claude-mem showed them to every checkout of that name: the import cannot
split what claude-mem kept under one name.

I6. **Exclusion and forget.** Unchanged and by the same names: the embedding phase passes over an
imported document of an excluded repository's project (milestone 4 D13), and work state goes to no
provider (L8). Milestone 5's forget of a repository is to reach its imported documents and work
state by the same rule. Its capture exclusion keeps an excluded repository's imported lists out
by that rule, as it refuses the repository's writes (docs/work-state.md L8): the rows are counted
(`never_recorded`), and since claude-mem still holds them, an import after the undo brings them
(slice 4a, Codex on #489).

I7. **Size.** The owner's database of 2026-09-24 (178,370 documents) made raw.db 323 MB and
knowledge.db 864 MB in an evaluation home (docs/milestone-4.md); their vectors add about 930 MB,
from the local model on this PC and from Workers AI on the iMac (decision 35's estimate).

I8. **Exclusions (parity row 14).** claude-mem's exclusion patterns, the comma-separated
`CLAUDE_MEM_EXCLUDED_PROJECTS` of the `settings.json` beside its database, come in as patterns never
recorded (docs/settings-privacy.md, Never record), before any row, each one only if no exclusion op
ever named it: one taken back on the page or with `--undo` is not added again by a later import. A
database with no settings beside it, or settings without the key (or with it `null`), imports as
before. The file is read as claude-mem reads it (`settings-document.ts`): a leading BOM skipped, and
the keys under `env` when that holds claude-mem's keys. A file that is there but cannot be read, is
not a JSON object, or holds the key as anything but one string stops the import before any write,
since the folders its patterns name would be recorded; the error names the file, its line and column
at most (Codex on #563). The import's line counts the patterns it adds (`exclusions`). An empty
entry is none, as claude-mem reads the list; a pattern oboete refuses (over 256 bytes, or with a
control character) is not brought over, and since the folders it names are then recorded, the line
counts it (`exclusions_refused`) and a warning on standard error says so by its count alone, to add
a shorter form with `oboete capture exclude --pattern`.

## Tests

1. A database with 13.28.0's tables and one with 13.35.0's (`notes`, `merged_into_project`,
   `work_state_entries`) both import; one at v65 or v67 comes in as at v64; one naming v68 is refused
   with nothing written.
2. A table is found by its columns, whatever version row names it.
3. Notes come in as the last line; a merged project's rows come in under the project they were
   merged into.
4. Work state rows come in as ops, oldest first, gated, with their times; a row the checks refuse
   is counted; a second run adds nothing.
5. A repository reads its project's imported lists, a worktree project's too, and not another
   project's, with its own writes by time: its own later write comes last though it reached the
   store first; an imported entry keeps its own time. An imported entry dated ahead of this
   device's clock lifts no write's clock, and a write made after it still comes last. A native
   entry under a key shaped like a worktree project's is read by that key alone.
6. The default home takes the import without `--eval-store` and refuses it with it.
