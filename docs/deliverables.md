# Deliverables: copies of what a turn made (parity rows X23, G01, X16; decision 73)

claude-mem 15.3 to 15.9 keeps copies of the files a Claude Code turn made, so a report written to a temp
folder or a page git ignores can still be opened from My Work after the original is gone. oboete does
the same (decision 73), with its own capture point, removal and serving. Read against claude-mem
v15.9.0 (`src/cli/deliverables-capture.ts`, `src/shared/deliverables.ts`,
`src/shared/deliverables-transcript.ts`, `src/services/sqlite/deliverables.ts`,
`src/services/worker/http/routes/WorkViewRoutes.ts`, `src/services/work-view/work-product.mjs`,
`src/services/work-view/request-projection.mjs`, `src/ui/my-work/src/components/request-card.tsx`;
Apache-2.0).

## What claude-mem does

- **When.** Its Stop hook (and SubagentStop, row X17) reads the turn from the transcript: the last
  user prompt's turn, the Write, Edit, MultiEdit and NotebookEdit calls and the work_state_write
  calls whose result was not an error. Nothing is copied for a turn whose prompt is wholly private,
  an excluded project, a worker on another host, or with `CLAUDE_MEM_DELIVERABLES_ENABLED=false`.
- **Which files.** Declared: a work_state_write field `work_product`, `work_products`,
  `deliverable`, `deliverables` or one ending in `_url` names any file (a path, or a JSON string of
  `{title, url|path|link, kind}` items); another field names a file only of a known kind. Inferred: a
  file the turn wrote of a known kind, outside the source tree (in no repository, or git ignores it;
  git failing or timing out counts as inside). Known kinds: `.png .jpg .jpeg .webp .gif .svg`
  (image), `.mp4 .mov .webm` (video), `.pdf .docx` (document), `.pptx .key` (deck), `.xlsx .csv`
  (spreadsheet), `.html` (page); `.md .markdown .txt .rst` count only when declared. A file must
  resolve (realpath) to a regular file inside the checkout or the temp folders, at most 512 MiB; at
  most 50 a turn, declared first.
- **Where.** `~/.claude-mem/deliverables/<project>/<stem>-<first 8 hex of sha256><ext>`, copied to a
  temp file, hashed, renamed; an existing name is reused. A row per session and copy records title,
  kind, source path, bytes, sha256, whether it was declared, its list and field, and when.
- **Shown.** My Work matches a copy to the request of its conversation; the card shows up to three of
  a request's products other than commits and branches, the copies among them, and "+N more"; the
  request's drawer lists them all. A copy replaces the record of the same path while the source is
  gone or unchanged since the copy. A copy no longer on disk is left out.
- **Served.** `GET /api/deliverables/:id/:filename`, loopback only, refused from another site, with
  `Content-Security-Policy: sandbox allow-scripts`, `nosniff`, `no-store` and a type by extension.
- **Removed.** Never: no size cap, no expiry, and deleting a session keeps its copies.
- **The rule.** A second attach line tells the agent the copies are made and linked (row G01).

## Decision 73 (2026-11-04)

The files are copied as they are, as claude-mem copies them: an exception to section 2's rule that
every stored byte is scanned and that binary content becomes a marker. The copies are never searched,
never sent to a model, never synced or exported, are opened only from this computer's browser, and go
when the records of their turn are forgotten; retention's expiry removes none, as claude-mem removes
none.

## Design

### When and what (slice 1)

- **At a turn's end, in the worker.** The Stop hook has 5 s and records the reply; copying up to 50
  files of up to 512 MiB there would hold the agent. A worker phase (`deliverables::run`, beside the
  shortlist, not under `[summary] curate`, skipped while a command asks the worker to step aside)
  reads each new `reply` or `end` record of a live hook session (`source = "hook"`: a replay's paths
  are not this machine's) and the turn's records before it (`Raw::turn_start`). A Codex turn may
  leave no reply (capture.rs: an empty Stop and transcript tail), so its records are read with the
  next reply or the session's end. A resident worker sees a reply within a second or two, so the copy
  is the file as the turn left it; a worker that comes later copies the file as it is then (a
  difference: claude-mem copies in the Stop hook).
- **Where it starts.** Its own checkpoint per device in deliverables.db (`deliverable_scan`), the seq
  and the time of the last turn end it took. The first run starts at the device's newest record: no
  history is copied. When raw.db no longer holds that record at that seq with that time (a restore,
  whose seqs are given again: raw.db ends below it, or another record's time is there), the
  checkpoint moves to the newest record: nothing older is copied. A record forget removed at that
  seq keeps the checkpoint (its seq is not given again).
  Copies are made outside any raw.db
  or deliverables.db transaction, a row written after each; between files the phase stops when a command
  asks the worker to step aside, and the checkpoint passes a turn only once all its files are done
  (a turn taken again repeats nothing: names are by content and rows are unique).
- **A private turn.** A prompt that is wholly private leaves no record (capture.rs: so whether a turn
  is recorded never depends on the settings), and a turn before it may leave no reply, so the records
  alone cannot tell its calls from the turn before. So the mark goes into the turn's own records:
  the prompt hook sets the session's private-turn flag (a file under `state/private-turns/`, no
  text), and while it is set each event the hooks record for that session (its tool calls, its
  reply) carries `"private_turn": true` in its body. The session's next public prompt clears it,
  as do its reply (after the reply is recorded with the mark) and its end. The hooks of one session
  run one after another (none is async), so the flag is set and cleared in the order the agent
  works; and as the mark is in the record, a busy store's write-back (docs/unwritten.md) and a
  restore keep it with the record. The phase copies no call whose record is marked, as claude-mem
  copies nothing for a wholly private turn. A flag that cannot be read marks the event (fail
  closed); a flag that cannot be set is reported as the hook's failures are (`oboete doctor`), and
  that turn's files may be copied: an accepted limit. Only agents whose prompt
  hook sees each prompt as it comes are copied from (Claude Code, Codex, Grok): agy's records are
  built from its transcript afterwards, and Cursor's print mode runs its tool hooks without its
  prompt hook (hook.rs), where no flag can be set. A prompt kept as
  `{"omitted":true}` (`[capture] store_prompts` off) is a public prompt. Muted and excluded sessions
  have no records to read.
- **The calls.** Write, Edit and MultiEdit (`file_path`) and NotebookEdit (`notebook_path`) records
  not `failed`, and oboete's `work_state_write` (as `curate::served` names it, any agent) with its
  `fields`. An input cut at the field cap is read for its path by its head (`{"file_path":"…`). A
  path a masking rule changed does not resolve, so it is not copied.
- **The files.** claude-mem 15.9's capture rules above (`deliverables-capture.ts` 108-153, not the
  15.1 display rules `work_product::from_fields` ports: a declared `~/` or relative path resolves,
  and another field names a known kind only), with its kinds (`FILE_KINDS`, the same list), title and
  kind rules (`title` or `name` cut at 140, a declared `kind` up to 140) and git checks (5 s each,
  cached per folder). The roots are the record's `cwd`, `/tmp` and the worker's own temp folder: an
  agent's own `TMPDIR` is not recorded, so a file only there is not copied.
- **The copy.** The source is opened once, without blocking (a FIFO put in its place cannot hold
  the worker), and everything is checked on that handle: a regular file, at most 512 MiB, read with
  that bound; its real path, read from the handle (`/proc/self/fd` on Linux, `F_GETPATH` on macOS,
  `GetFinalPathNameByHandleW` on Windows; another system copies nothing), inside a root. So a path or
  an ancestor swapped for a link before or after the check is not followed. The folder must be a real
  directory inside `<home>/deliverables/` (no link), the copy is staged under a new name there
  (create-new), hashed as read, and renamed.
- **The folder.** `<home>/deliverables/<name>/`, `<name>` the repository's display name
  (`manifest::repo_name`, the last part of its key) made safe as claude-mem's `safeName` does, or
  `unknown-project` without one; names as claude-mem's, `<stem>-<8 hex><ext>`. An existing name is
  reused only when its whole sha256 is the copy's; another file under that name (two contents whose
  hashes start alike) leaves the copy named by its whole hash, its stem cut so the name stays within
  255 bytes.
- **The record.** A row in `deliverables.db` beside raw.db (not providers.db, which a home that
  curates nothing never makes), device-local, never synced and untouched by a rebuild or a restore: device, agent, session, repo, the turn end's and the call's seq and time, title, kind,
  source path, the copy's path under the folder, bytes, sha256, declared, list, field, and when it
  was copied (the copy is the file as it was then, best effort: a later turn may have changed it).
- **The setting.** `[capture] deliverables` (on by default), as claude-mem's is on; the settings
  page's switch comes with the next Settings PR (#823), as every setting has a path there
  (docs/settings-redo.md).

### Removal (slice 1)

- Forgetting removes copies, by forget's own durable denials, as raw.db refuses a forgotten record
  arriving again: a row is forgotten when `denied_records` holds the origin of its call or its turn
  end (`forget::live_origin` of device, seq and time: a seq given again after a restore is another
  record), or a `denied_scopes` page covers it (its session, its repository on its device, or a time
  range on its device, registered after its time). A record merely not found (a restored raw.db) or
  expired forgets nothing; a later forget of its session or repository still does, by the scope.
- Read paths decide by those denials from the moment forget is registered (spec: every read
  filters from then): slice 2's listing and every file request ask them before answering.
  `deliverables::sweep` is cleanup: it deletes forgotten rows, then each file no row names, only
  regular files directly in a folder of `<home>/deliverables/`, no link followed. It runs after
  forget's purge (step 5, under its lock) and in the worker at its idle time (its scan for files no
  row names only there, where no copy is in progress), so a sweep that failed is made up by the
  next.
- Retention's expiry (`[retention] days`) removes no copy (decision 73): claude-mem removes none, and
  a copy is what was made, kept longer than the text of the records about it.

### The rule (slice 2)

- G01's second line, as claude-mem 15.3 wrote it, naming oboete and its folder, once My Work links
  the copies (its text says they are linked on the request's card).

### Served and shown (slice 2)

- **Links.** A copy is opened by a link that is its own capability:
  `/files/<HMAC-SHA256 of its id under a secret the viewer makes when it starts>/<file name>`. The
  page's token stays in its header; a link opens one copy, on this computer, until the viewer
  restarts. The file must resolve inside `<home>/deliverables/`. Headers as claude-mem's
  (`Content-Security-Policy: sandbox allow-scripts`, never with `allow-same-origin`, and not the
  page's own policy; `nosniff`; `no-store`) and `Referrer-Policy: no-referrer`: a copied page's
  script runs in an opaque origin and cannot read the page's token. A link resolves only its own
  row, does not depend on the token (a new token keeps it; a restart ends it), is refused once its
  row is forgotten, and its bytes are streamed from the file, not held whole in memory.
- **My Work.** A request's copies join its products in what the page is answered with (the
  viewer's projection, not `my_work::products_read`, which also feeds My Work's model jobs: a copy
  and its link never reach a model), a copy replacing the record of the same path as claude-mem's
  does; the list answer gives each request up to three products other than commits and branches,
  and how many more, for the card. A copy is opened, hashed and served only as a regular file
  checked on its handle.

### Not built here

- SubagentStop's copies come with row X17 (oboete installs no SubagentStop hook yet).
- Codex and Grok write files through `apply_patch` and shell commands, which claude-mem does not read
  either: their declared files are copied, their written ones are not.

## Acceptance (slice 1)

- A turn that writes `report.pdf` in a folder git ignores and declares `notes.md` as `work_product`
  copies both into `<home>/deliverables/<repo>/`, named by content, with one row each; `src/a.rs`
  written in the repository is not copied; a second turn writing the same bytes reuses the copy.
- Nothing is copied for a failed call, a path outside the checkout and the temp folders (a symlink
  leading out too), a file over 512 MiB, a turn without a prompt record, a replayed session, the
  setting off, or the 51st file of a turn; a git that fails counts as inside.
- A Write whose input was cut at the field cap is copied by the path at its head; a Codex turn with
  no reply is copied at the next reply or the session's end; a public prompt, a turn without a reply,
  then a wholly private prompt whose turn declares a file: the file is not copied, also when the
  public prompt is written back late from the unwritten spool, when another session's turn ends in
  between, and after a restore gives seqs again; an unreadable flag marks the event; the next public
  prompt's calls are copied again.
- A file, or a folder above it, swapped for a link to outside the roots is not copied; a FIFO in a
  file's place is refused without blocking; a folder of `<home>/deliverables/` that is a link is not
  written to; a file already under
  the copy's name with other bytes is kept, and the copy takes its whole-hash name.
- Forgetting the session removes its rows and files; a file another row names stays; a row whose
  records are not found (a restored raw.db) and a row whose records expired keep theirs; after a
  restore gives a copy's seqs to other records, forgetting those keeps the copy; forgetting the
  session later still removes it. After a restore the checkpoint moves to the newest record.
- The first run copies nothing of the records already there; a step-aside between two files leaves
  the turn to the next round, which copies the rest once.
- MultiEdit's and NotebookEdit's inputs are checked against real records before their tests are
  pinned.

## Acceptance (slice 2)

- A copied HTML page's script cannot read the page's token or call the page's API.
- A link given before a forget is refused right after the forget is registered, with its purge not
  yet run.
- The rule's second line is shown with the first, and the short rule keeps neither.
