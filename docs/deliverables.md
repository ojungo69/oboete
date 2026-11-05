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
  are not this machine's) and the turn's records before it (`Raw::turn_start`), one at a time
  (`Raw::turn_seqs`, `Raw::event_at`), keeping only what a copy needs of each (#836). A Codex turn may
  leave no reply (capture.rs: an empty Stop and transcript tail), so its records are read with the
  next reply or the session's end. A resident worker sees a reply within a second or two, so the copy
  is the file as the turn left it; a worker that comes later copies the file as it is then (a
  difference: claude-mem copies in the Stop hook).
- **Where it starts.** Its own checkpoint per device in deliverables.db (`deliverable_scan`), the seq
  and the time of the last record it read (a turn end it took, or one it read past), and raw.db's
  file then (`Raw::file_identity`: device and inode, the volume and file index on Windows). The first
  run starts at the device's newest record: no history is copied. When raw.db no longer holds that
  record at that seq with that time (a restore, whose seqs are given again: raw.db ends below it, or
  another record's time is there), the checkpoint moves to the newest record: nothing older is
  copied. A record with no time at that seq (a tombstone, or one a forget removed since) is the same
  record only in the same raw.db file, which a restore replaces (#836 item 3); a checkpoint kept
  before the file was is taken as it is and given the file at its next run, also when no record
  follows it. A record forget removed at that seq keeps the checkpoint (its seq is not given again,
  and the file stays).
  Copies are made outside any raw.db
  or deliverables.db transaction, a row written after each; between files the phase stops when a command
  asks the worker to step aside, and the checkpoint passes a turn only once all its files are done
  (a turn taken again repeats nothing: names are by content and rows are unique).
- **A private turn.** A prompt that is wholly private leaves no record (capture.rs: so whether a turn
  is recorded never depends on the settings), and a turn before it may leave no reply, so the records
  alone cannot tell its calls from the turn before. So the mark goes into the turn's own records:
  the prompt hook sets the session's private-turn flag (a file under `state/private-turns/`, no
  text) for a wholly private prompt, and while it is not set each tool call and reply the hooks
  record for that session carries `"public_turn": true`, added after the redaction gate (a rule
  cannot take it away at capture; a rescan that masks it later only stops a copy). The session's
  next public prompt clears the flag, as does its end; its reply does not, since a turn the harness
  starts next often goes on with the private work. A call counts only when its record is so marked
  and the record that started its turn comes before it, a `prompt` or the harness's message
  (`envelope`: a task's notification, another session's message), which neither sets nor clears
  the flag. So a turn the harness starts after a private turn is private until a public prompt,
  where claude-mem, whose transcript reading takes such a message as a public turn of its own,
  copies its files. Hooks set up in the middle of a turn copy nothing of it. The hooks of one
  session run one after another (none is async), and
  the mark is taken as each event comes and kept in its record, so a busy store's write-back
  (docs/unwritten.md) and a restore keep it with the record. A flag that cannot be read leaves the
  event unmarked (fail closed); a flag that cannot be set is reported on the hook's error output, and
  that turn's files may be copied: an accepted limit. Only agents whose prompt
  hook sees each prompt as it comes are copied from (Claude Code, Codex, Grok): agy's records are
  built from its transcript afterwards, and Cursor's print mode runs its tool hooks without its
  prompt hook (hook.rs), where no flag can be set. A prompt kept as
  `{"omitted":true}` (`[capture] store_prompts` off) is a public prompt. Muted and excluded sessions
  have no records to read.
- **The calls.** Write, Edit and MultiEdit (`file_path`) and NotebookEdit (`notebook_path`) records
  not `failed`, Grok's `search_replace` and `write` (`file_path`), and oboete's `work_state_write`
  (as `curate::served` names it, any agent) with its `fields`; Grok's hooks give an MCP call's input
  as `{"tool_name", "tool_input"}`, read inside `tool_input`. An input cut at the field cap is read
  for its path by its head (`{"file_path":"…`). A path a masking rule changed does not resolve, so
  it is not copied. A call whose record has no absolute `cwd` names nothing: its roots would start
  at no folder.
- **The files.** claude-mem 15.9's capture rules above (`deliverables-capture.ts` 108-153, not the
  15.1 display rules `work_product::from_fields` ports: a declared `~/` or relative path resolves,
  and another field names a known kind only), with its kinds (`FILE_KINDS`, the same list), title and
  kind rules (`title` or `name` cut at 140, a declared `kind` up to 140) and git checks (5 s each,
  cached per folder, with `LC_ALL=C` so "not a git repository" is read in any language: #836). The roots are the record's `cwd`, `/tmp` and the worker's own temp folder: an
  agent's own `TMPDIR` is not recorded, so a file only there is not copied.
- **The copy.** The source is opened once, without blocking (a FIFO put in its place cannot hold
  the worker), and everything is checked on that handle: a regular file, at most 512 MiB, read with
  that bound; its real path, read from the handle (`/proc/self/fd` on Linux, `F_GETPATH` on macOS,
  `GetFinalPathNameByHandleW` on Windows; another system copies nothing), inside a root. So a path or
  an ancestor swapped for a link before or after the check is not followed. `<home>/deliverables/`
  and the folder are opened as real directories, no link followed, and on Linux and macOS held by
  their descriptors while the copy is made (#836 item 2): it is staged under a new name in the folder
  held (create-new, readable by the owner alone whatever the source's mode), hashed as read, and
  renamed there; an existing name is read there, and a staged file whose copy fails is removed there.
  A folder swapped for a link once it is held is not followed. On Windows these steps go by path
  after the same check: a link or a junction put there between the check and the act is followed (a
  stated limit).
- **The folder.** `<home>/deliverables/<name>/`, `<name>` the repository's display name
  (`manifest::repo_name`, the last part of its key) made safe as claude-mem's `safeName` does, or
  `unknown-project` without one; names as claude-mem's, `<stem>-<8 hex><ext>`. An existing name is
  reused only when its whole sha256 is the copy's (read no further than one byte past the copy's
  size, so a larger file there holds nothing up); another file under that name (two contents whose
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
  `deliverables::sweep` is cleanup: for each forgotten row it removes the file when no other row
  names it, then the row (a file it cannot remove keeps its row for the next sweep), only regular
  files directly in a real folder of `<home>/deliverables/`, no link followed, the folder held by its
  descriptor while its files are read and removed (Linux and macOS, #836 item 2). It runs after
  forget's purge (step 5, under its lock) and in the worker at its idle time (its scan for files no
  row names only there, where no copy is in progress; a folder it cannot read is said and passed
  over, the others swept), so a sweep that failed is made up by the next.
- Retention's expiry (`[retention] days`) removes no copy (decision 73): claude-mem removes none, and
  a copy is what was made, kept longer than the text of the records about it.

### The rule (slice 2)

- G01's second line, claude-mem 15.3's in oboete's words (`work_state::RULE_COPIES`): oboete copies
  when the turn ends, into this repository's folder under `~/.oboete/deliverables/`, and links the
  copy on the request's card in My Work, as claude-mem's line says (since slice 2b; its page lists
  them all). It is shown at session
  start only to an agent whose files are copied (Claude Code, Codex, Grok) while `[capture]
  deliverables` is on; `oboete inject` and the viewer's Context page, which OpenCode and Pi read,
  leave it out. The least size keeps neither attach line (`SHORT_RULE`).

### Served and shown (slice 2)

Slices 2a and 2b (as built) serve the copies, list them on a request's page and put them on its card
with row X16's products; slice 2c takes #836's first three items: items 2 and 3 as built (the
destination held by its descriptors, the checkpoint known by raw.db's file, above), item 1 (the
call's folder as a root by its identity, recorded by the hook) in a PR of its own.

- **Links.** A copy is opened by a link that is its own capability: `/files/<tag>/<name>`, the tag
  the first 16 bytes of an HMAC-SHA256 (`hmac` 0.12) under a secret the viewer makes when first
  needed, over the row's whole provenance, each field length-prefixed (device, id, turn seq and
  time, call seq and time, sha256, copy name): a row id a sweep frees and another copy takes names
  another tag unless the turn, the call and the name are the same too. The name is for the browser
  alone. No token is needed (a new token keeps a link; the viewer's restart ends it); the reads'
  refusals hold (a method other than GET or HEAD, a body, another host), and so does claude-mem's
  of a cross-site fetch (`Sec-Fetch-Site: cross-site`). Every row's tag is compared in constant
  time (`verify_truncated_left`). deliverables.db is read through the home resolved and its own
  name not followed (`deliverables::open_read`, as `raw::read_only` opens raw.db: SQLite's NOFOLLOW
  refuses a link anywhere in the path, so a home reached through one would read nothing).
- **The file.** `<home>/deliverables/<copy>`, the home canonicalized once and the fixed folder
  joined to it: both folders real directories, the file opened without following a link or
  blocking, a regular file whose handle is at that very path (a `deliverables/` swapped for a link
  is refused), of the row's size, and whose bytes, hashed on the handle before any header, are the
  row's; then read again from its start on the same handle. A copy not as its row says (gone, a
  link or not a regular file in its place, other bytes) answers 404; one that could not be read in
  time or at all, 503.
- **Admission.** Then raw.db is opened (`raw::read_only_until`, none when there is no store:
  nothing is served), the readers' hold taken (`dispatch::reading`, the order a restore and a
  forget's registration take them), and the call's and the turn end's denials read in it, each
  wait (a restore's swap, the hold, SQLite's while busy, set again before each query) within what
  is left of the deadline when it starts
  (`deliverables::denied`, the query `Raw::forgets` runs): a forget registered before then refuses
  the link (404) from that moment, its purge run or not. The head is sent in that hold, and both are
  let go before the body; a forget registered after it waits for the hold, and a stream it let in
  finishes with the bytes it was let in with.
- **The answer.** Its own head (not the page's): the type by the copy's extension (claude-mem's
  table), `Content-Length`, `Content-Security-Policy: sandbox allow-scripts` (never with
  `allow-same-origin`, and no policy of the page's own), `nosniff`, `no-store`, `Referrer-Policy:
  no-referrer`, `Accept-Ranges: bytes`: a copied page's script runs in an opaque origin, so it can
  read neither the page's token nor an answer of the API. The body is streamed in 64 KiB chunks.
  One deadline (`FILE_TIME`, 60 s) holds for the whole answer from when its head came: the lookup,
  the hash, the admission's waits and every write; a reader slower than that is cut, and a player
  asks again from where it stopped. A refusal the deadline made (503) is still written, within a
  second. A HEAD has the whole length; on a GET one `Range: bytes=a-b` (or `a-`, `-n`)
  answers 206 with `Content-Range`, several or a malformed one the whole file, one past the end 416.
- **My Work.** A request's copies join its products on its page (`my_work::products`, the viewer's
  answer, not `products_read`, which the model's progress review reads: no copy or tag reaches a
  model). A copy is a request's when the request holds its conversation whole, or holds a prompt
  span with the copy's call while raw still has that call (the record at its seq has its time); a
  copy whose call raw no longer has (a restore gave its seq to another record) joins only the
  conversation's one holder among all requests built, counted before the answer's range. A copy a
  forget denies (`Raw::denies`: its call or its turn's end) is none from the registration on, its
  sweep run or not, and its request's `copies` changes with it; the page's answer is built in the
  readers' hold (#439), so a registration under way is waited for, not met halfway. Each copy
  is `{kind, title, url: null, path, at, file}`, `file` its link's tag. A copy whose file is not on
  disk as its row says (`deliverables::on_disk`, claude-mem's `bucketFileAvailable`: opened as
  `open_copy` opens it, a regular file of its size whose handle is at its very path) is left out and
  hides nothing (slice 2b). One on disk hides the
  recorded file of the path it covers (claude-mem's `copyCoversSource`: the source gone, or not
  changed after the copy), its own path compared with the path as recorded, before the page's gate
  (#840): a card's file as the card stored it, a work item's where the gate left its field as it
  was (a field it changed is read as it shows it, so its paths hide nothing); never a path that
  holds the mask, which a gate made when the record was captured or summarized. So a copy whose own
  path is the very text the gate shows of a recorded path hides nothing, and a copy of a card's file
  a rule masks hides its record. Newest first within the 60. `file`, each request's `copies` and the
  list's `instance` are the viewer's own values, kept out of the answer's gate walk as `version` is;
  titles and paths are gated.
- **Freshness.** A copy lands a second or two after its turn's reply, which moved the request's
  version. So the list answer gives each request `copies` (a short hash of its copies'
  provenance) and itself the viewer's links' `instance`; `/api/version` moves with a hash of every
  row's provenance and with that instance. The page keeps a request's products by its id, version,
  `copies` and the instance, and at each list read compares the key the answer gives with the one
  it last read by: the open page's products are read again in place, the other updates held behind
  "Show updates" or not. A products read that failed is asked again at the next poll, the version
  moved or not, three times at most for one key; one the viewer refused (a request no longer
  listed) waits for the next key; an answer whose copies were not read (`copies_unread`: their
  store did not read) is shown and asked again the same way. A list read begins a key's read once:
  not again while it is under way, so its answer lands, nor after it failed; back at the key that
  was read, it drops a read under way for another, whose answer would replace it. A drawing shows
  what was read for its request and begins a read only for a request none was begun for, as the
  row it draws may be older, behind "Show updates". A copies' store that does not read leaves the list and the page
  without copies, not without their requests, and the version then moves with the rest alone.
- **Card (slice 2b).** The list route's answer (`my_work::listed`) gives each request with any
  deliverables its card: `deliverables`, the first three of its products as its page builds them
  (`shown_products`, the one builder of both) that are not history, as claude-mem's
  `isCardDeliverable` takes them (commits and branches stay on the page), copies among them with
  their tags; and `deliverable_count`, how many of the 60 are such. They are built in the readers'
  hold with the copies' fingerprints, from the same read of the copies: a forget registered before
  it leaves its copies out, and one registered meanwhile waits for the cards. On a copy of this
  PC's store (53 requests, 12 with a card) they add 0.05 s to a list answer of 0.41 s (release,
  median of 7), the list read each minute while My Work is open; a request's product calls are
  read by device, 100 at a time (`Raw::at`), where one at a time added 0.12 s. A request whose
  products do not read has no card, and a copies' store that does not read leaves the cards
  without copies. Each tag leaves the gate's walk as the page's do. The card shows them under its
  progress and above its focus chip, as claude-mem's under its terminal and above its next move:
  "Deliverables · N" (成果物 · N); the three as chips, each with claude-mem's icon of its kind
  (lucide's, drawn as inline SVG), a copy or an http or https address a link opened apart from the
  page, any other as text, its hint its path or else its whole title; and "+N more" (ほか N 件,
  named with the words it shows), which opens the request
  as its title does. Cards only, as claude-mem's; the page's section is "Deliverables" too, the word
  of claude-mem 15.9's drawer. A viewer restarted while updates wait behind "Show updates" leaves the
  drawn chips' links to the old one until they are shown: meanwhile they open nothing (404).

### Not built here

- SubagentStop's copies come with row X17 (oboete installs no SubagentStop hook yet).
- Codex writes files through `apply_patch` and shell commands, and Grok through shell commands as
  well as its `search_replace` and `write`; claude-mem reads neither `apply_patch` nor a shell
  command: a file so written is copied only when the turn declares it.

## Acceptance (slice 1)

- A turn that writes `report.pdf` in a folder git ignores and declares `notes.md` as `work_product`
  copies both into `<home>/deliverables/<repo>/`, named by content, with one row each; `src/a.rs`
  written in the repository is not copied; a second turn writing the same bytes reuses the copy.
- Nothing is copied for a failed call, a path outside the checkout and the temp folders (a symlink
  leading out too), a file over 512 MiB, a turn with neither a prompt nor an envelope record, a
  replayed session, the setting off, or the 51st file of a turn; a git that fails counts as inside.
  A turn a task's notification starts (an envelope) copies its files as a prompt's turn does.
- A Write whose input was cut at the field cap is copied by the path at its head; a Codex turn with
  no reply is copied at the next reply or the session's end; a public prompt, a turn without a reply,
  then a wholly private prompt whose turn declares a file: the file is not copied, also when the
  public prompt is written back late from the unwritten spool, when another session's turn ends in
  between, and after a restore gives seqs again; nor is a file a turn the harness starts after the
  private one declares, its reply notwithstanding; an unreadable flag leaves the event unmarked; a
  call before the prompt or envelope record of its turn, or whose mark a rule masked, is not
  copied; the next public prompt's calls are copied again.
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
- A card shows the first three of what its request's page lists that are not commits or branches,
  and how many; "+N more" opens the request; a copy whose file is gone is left out and hides
  nothing; a copy whose own path is the masked text of a recorded path hides nothing (#840).
- A folder of `<home>/deliverables/` swapped for a link to a folder outside once a copy or a removal
  holds it (Linux, macOS): the copy is made, an existing name read and a failed copy's staged file
  removed in the folder held; a removal and the sweep's walk of files no row names remove from the
  folder held, and read its names; nothing outside is read, written or removed. `deliverables/`
  swapped for a link before a missing folder is made: the folder is made in the one held. A folder
  that is a link already is neither written to nor emptied (#836 item 2).
- A checkpoint at a record with no time, then a restore below it that gives its seq to another such
  record: the restore is seen, the checkpoint moves to the newest record and a turn after it is not
  copied; in the same raw.db file that turn is copied. A checkpoint kept before the file was is taken
  once and given the file, also with no record after it, so a later restore is seen (#836 item 3).
