# claude-mem's history in the everyday store (N1)

Row N1 of the parity plan (docs/claude-mem-parity.md) and G54 of the 13.34.2 comparison:
`oboete import claude-mem <db>` brings the owner's claude-mem database into the store the hooks
write, its work state with it (docs/work-state.md L10), before claude-mem is removed (decisions 35
and 37). Until now it imports into evaluation homes only (`--eval-store`, docs/pr-b.md:21).

## Why the guard can go

docs/pr-b.md kept the import out of the everyday store because 160,000 rows that no repository's
injection could find would have come in before repositories mapped onto claude-mem's project
names. Since then imported documents are injected through the session they were recorded in
(spec 4.5, #506) and, since decision 52, by the checkout's claude-mem names, and search finds a
repository's imported documents by name: those of the claude-mem project its key ends in, and of
that project's worktree sessions (`imported_repos`, milestone 4 Task 4). Work state needs the same
rule and nothing more.

## Rules

I1. **Where.** Without `--eval-store` the import goes into the home given, the default one
included. `--eval-store` keeps its meaning, an evaluation home: it still refuses the default home.

I2. **Which databases.** claude-mem's schema from 13.28.0 (migrations to v52) to 15.15.0 (v74).
The importer reads what it finds by table and column (`PRAGMA table_info`), not by version number:
another claude-mem draft (#4341) also numbers a migration v61. v65 (13.36.0,
`refreshUnicodeTitleNormKeys`) only recomputes `observations.title_norm_key`, the key claude-mem
finds a duplicate title by, which the import does not read; 13.36.0 to 13.39.0 change no other
table or column. v66 (13.40.0) adds cloud sync columns to `work_state_entries` (`synced_at`, the
origin device and row, `sync_rev`) and v67 lets the sync ledgers name the work state kind; the
import reads neither. 13.41.0 to 15.0.1 add only work state telemetry receipts and, in a database
of their own, My Work's reviews, neither of which the import reads. v68 (15.0.2) adds
`observation_files`, an index of each observation's `files_read` and `files_modified` for
claude-mem's lookup by file; the import reads those lists from the observation itself (I9), not
from the index. v69 to v71 (15.7.0) and v72 (15.12.0) add the tables of kept images
(`media_events`, `media_attachments` and their links to observations and events): an image is a
file under claude-mem's `media-v1` folder, and an observation's text is as it was. The import
reads neither the tables nor the files, so claude-mem's kept images stay where they are. v73
(15.12.0) adds `committed_observation_tool_uses`, the tool uses whose observations were stored,
so a transcript read again after a restart stores none twice, and the records of deleted sessions
(`deleted_content_sessions`, `deleted_memory_sessions`); v74 (15.12.0) adds triggers that keep a
deleted session's earlier content from coming back. A deleted session's rows are gone from the
tables the import reads, and the import reads none of the three. A database whose `schema_versions` names a
version above 74 is refused, with nothing imported: a newer claude-mem may keep what this import
would leave behind. The newest version is the highest one named, wherever its row is: the media
migration runs after the others, so v72 is stamped after v74. The message names both versions.

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
from the local model on this PC and from Workers AI on the iMac (size: docs/milestone-4-plan.md
"What needs the owner" item 6; Workers AI cost: decision 35's estimate).

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

I9. **The files an observation names (parity row 8, 2026-10-17).** An observation's
`files_read` and `files_modified` come in as a files op beside its document, for the note on a
file: an `import` op with the document's labels and the two lists, gated, and no text, which no
reader of documents takes for one (docs/file-note.md F10, F11). Each import also brings the
files op of an observation imported before that has none under its document's repository, and
`oboete import claude-mem <db> --files` brings only those, with no new document, work state or
exclusion; a document claude-mem moved since stays where it is filed and gets its files op
there. The import's line counts the observations whose files came in (`files`, none of a
forgotten or denied uid) and the paths left out to keep a files op within 32 KiB (`files_cut`; an
observation none of whose paths fits gets no files op, so each run counts its paths again). Under
`--files`, `seen` counts the observations imported before; those claude-mem recorded since are
left to the plain import and not counted.

I10. **The gate (2026-11-12).** The documents, the files ops and the work state the import stores
pass the gate with the home's rules (the exclusion patterns of I8 come in as claude-mem holds
them). A prompt keeps its first 8,000 characters (a live prompt keeps 64 KiB; decision 105 keeps
the cut as it is): `hook::clip` masks
it with the bundled rules, so that its check for a key block the cut leaves open sees every
header, and the home's rules then gate what it kept, as they do a title and a body. Until then a
prompt was masked with the bundled rules alone. Before the bundled rules mask the window, the
home's rules read all of the prompt as it came (`hook::read_whole`, #994), as capture's scan
reads a live prompt, and what they find is masked there; the prompt is read again with those
masks in until nothing new is found, as a scan's rescan does. So a value the cut falls inside,
one that ends far past it, one whose rule needs the text after it, one that holds a value the
bundled rules mask and one a rule finds only behind another rule's mask are each one mask
before the cut (their parts used to stay), and the cut keeps 8,000 characters of the masked
text. A prompt that is not cut is masked as capture masks a live one, by all of the rules at
once (`redact::scan`). A prompt in which
one of the home's rules matches more times than a scan holds, or that is one value whole, is one
mask, as capture stores a live prompt with that many; a bundled rule's matches are a mask each,
however many. The marker that opens a key block (`-----BEGIN`) is left as it is where a rule of
the home's finds it with no closing marker after it in the same match, and the rest of that
match is masked: the check for a key block the cut leaves open must see the header, and a rule
that matched its `BEGIN` alone, or a value the header stood in, would hide it. That check, and
this, find the markers in whatever case they are written (2026-11-14; upper case alone was
found), and what is kept ends at the first header that no footer follows (the last header was
looked at alone, so a second open block kept the first one's lines). A block whose footer is
past the window is one mask under a rule of the home's, which reads it to its footer; under
none it is dropped from its header, as before. Under a rule of the home's the `<private>`-style
blocks are taken out until none is left before the rules read: a mask put in a tag would keep
the gate's own removal from finding a block that only a later removal finds. Under no rule of
the home's nothing is read
before the bundled rules, and what is stored is what was stored, but for a key block written in
lower case, or a first of two, that the cut leaves open, which now goes. A mask the cut falls inside is
kept whole (2026-11-14; its first characters used to stay). While
`[capture] store_prompts` is off no prompt's text is
stored, as a live prompt's is not: the rows are counted with the empty ones (`empty`) and come in
once the setting is on again. A label names something, so the gate must leave it as it is: a row
whose repository (`claude-mem:<project>`) or session id the gate changes, by a mask or by its
cap, is left out, its document, its files op and its work state alike, and the line counts it
(`masked`) at each run. So is the files op of a document filed before under such a label. A
document whose op is over the op's cap (an observation's or a summary's long body) is cut to fit,
and what that cut leaves is scanned once more with the same rules, as it is stored. What an
earlier import stored stays as it was stored.

## Tests

1. A database with 13.28.0's tables and one with 13.35.0's (`notes`, `merged_into_project`,
   `work_state_entries`) both import; one at v65, v67, v68 or v74 comes in as at v64, a path only
   v68's index holds in no file record, and v74's image links, deleted-session records and
   triggers in nothing; one naming v75 is refused with nothing written.
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
7. A prompt is gated with the home's rules too: a value only the home's rule finds is masked, in
   a short prompt and in one longer than is kept; under no rule of the home's the text stored is
   the one stored before, also for key blocks cut short and whole about the cut; while redaction
   is off the prompt is kept as typed. What a cut to the op's cap leaves is scanned again: a
   value a line-scoped allowlist passed over for the line's ending is masked once the ending is
   cut.
8. While `store_prompts` is off no prompt comes in; a run with it on brings them.
9. A row whose repository or session the gate changes is left out and counted, with its files op
   and its work state, and nothing stored holds the label; the files op of a document filed before
   under such a label is left out too, under `--files` and for a row whose project is lost.
