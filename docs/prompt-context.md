# Memory found by meaning at each prompt (G23)

Plan of the slice that widens what reaches the agent at each prompt, written before the code, as
the other slices' notes are. Decision 40 widened delivery at each prompt to cards and session
summaries found by meaning (G23 of the 13.34.2 comparison); decision 44 (2026-10-13, below)
settles when and how.

## What claude-mem does (13.37.0)

- At each prompt its hook (`hook claude-code session-init`) asks its always-on worker over local
  HTTP: `/api/sessions/init`, then `/api/context/semantic` with the prompt as the query
  (`src/cli/handlers/session-init.ts`, `src/services/worker/http/routes/SearchRoutes.ts`).
- A prompt under 20 characters gets nothing. Otherwise the worker searches the project's
  observations (its cards) by meaning and returns the first 5 (`limit`, at most 20), each as its
  title, date and narrative under "Relevant Past Work (semantic match)". There is no relevance
  cut: a project with observations always gets five. Summaries are not searched.
- The two calls share one budget inside the host's 15 s hook timeout (10 s by default on POSIX,
  14 s at most, 500 ms kept back; 2 s for Codex). A slow or stopped worker adds nothing.
- `CLAUDE_MEM_SEMANTIC_INJECT` is off by default; the owner has it on.

## What oboete does now

Spec 4.2 and 4.6: with `[inject] per_prompt` on (off by default), the prompt hook matches the
prompt's words (trigrams) against the session's shortlist of about 50 delivered claims, which the
worker builds ahead (with vectors where it has them), and injects the claims whose share of the
prompt's words reaches 0.53 (`shortlist::THRESHOLD`), under "Decisions that may bear on this
prompt". Claims only; imported history is never injected (4.5); the hook never embeds a query and
never waits on the network (4.1, decision 38's "Not now" for asking the resident worker).

## The change

1. **When.** At prompt submit, at the prompt point where the per-prompt block is built now (the
   agents' UserPromptSubmit, agy's PreInvocation; Grok's UserPromptSubmit keeps its picks for its
   next tool call, as now). A prompt under 20 characters, or a harness envelope, asks nothing.
2. **Who computes.** The resident viewer, which the resident worker keeps up, asked by the hook:
   `POST /api/prompt-context` on `127.0.0.1` at the configured port, with the token of
   `state/view-token`, only when the viewer holds `state/view.lock` and its outcome says it
   listens (`view::resident_up`). The request names the calling checkout's repository (and
   branch), as the hook labels it: the viewer has no checkout of its own and its other searches
   read every repository, so the repository is the search's scope and `Query::caller`, and a
   repository excluded from sending (spec 5.5) or never recorded (6.1) has its prompt embedded by
   no provider and gets the words side only. The hook loads no model and calls nothing off the
   machine. The viewer runs the search as it runs one for the page or the MCP server
   (`search::b`'s `Caller::Reader`): it embeds the prompt with the embedder set up (the local
   model, loaded on the first query and kept, on this PC; Workers AI on the iMac) through the
   egress gate, waits for that vector at most `QUERY_TIMEOUT` (1.2 s) and answers from words
   alone past it.
3. **What.** The repository's delivered claims, current cards, session summaries and imported
   history (decision 44's second answer), ranked as search ranks them: meaning and words fused
   (reciprocal rank fusion, spec 4.10; nDCG@10 0.545 against claude-mem's 0.244 on the owner's
   questions, decision 36); imported history by meaning only, as the second slice found (below). At
   most 5 (claude-mem's number), within `per_prompt_chars`, each as one dated line: a claim as now,
   a card by its title and the first sentence of its narrative, a summary by its request, an
   imported record labelled imported with its status unknown. Nothing already shown in the session
   (the shown set, as now). The prompt's own text is never injected.
4. **Below the threshold, nothing.** Each candidate passes on either side: its words' share of
   the prompt at today's 0.53, or its meaning's cosine to the prompt at `T`. Unlike claude-mem, a
   prompt nothing bears on gets nothing: text that does not bear on the prompt costs the agent's
   context. `T` before the one evaluation of decision 34 is set without model calls, from the
   Inject measurement's 50 no-answer dev questions (milestone 4 D18; nothing in the dev home
   answers them, so anything injected for one is irrelevant): the lowest `T` at which they pass
   D18's line (a one-sided 95% upper bound of 10% irrelevant: at most 1 of the 50 gets anything).
   The one evaluation sets both thresholds, with the false-premise questions, by the same line.
   `[inject] per_prompt` stays off by default until then (4.6); the owner turns it on, as the
   owner has claude-mem's on.
5. **One deadline for the prompt point.** `[inject] per_prompt_wait_ms` bounds the hook's wait for
   the viewer, counted from the hook's read fence, not from the request, so the time the words side
   takes counts against it. The hook builds the words side (as now but over the same kinds: the
   full-text index of claims, cards, summaries and imports, with the same 0.53 cut; imports left
   out as built, the second slice) while it waits for the viewer, so a late or missing answer
   leaves a block that is already built. The hook's own reads are local and not cut at the
   deadline, as the prompt point's reads were not before; they are measured (As built). The default
   comes from measurement, before the code merges: the prompt point's time (the fence, the viewer's
   answer with its query vector within 1.2 s, the search, the lines) on this PC's WSL with the
   local model, set so that its 95th percentile fits, and not above claude-mem's 2 s for Codex
   unless the measurement says so. The iMac's measurement waits for resident mode there, and oboete
   goes on the iMac when agents are used there (decision 47). No resident viewer, no answer by the
   deadline, or an error: the words side's block. A meaning answer that comes late is lost for that
   prompt; this is the known cost.
6. **Fail-open.** Nothing on this path waits for the viewer past the deadline, and it writes no
   memory and no index: the answer is read, gated (`outbound`, as every block), fenced and shown.
   What it writes is what a search and a prompt point write now: a query sent to a provider is
   reserved and settled in `providers.db` (the embedding budget), and what is shown joins the
   session's shown set, so it is not shown again. The hook latency lines (milestone 2's write
   line, milestone 4's read line) are measured again with it.

## As built (the first slice)

- **What.** Claims, cards and session summaries. Imported history came with the second slice
  (below): #506 wants it only through an explicit mapping of the import's documents to
  repositories, and the import reads a project by its name (docs/claude-mem-import.md I5).
- **What is sent.** Only a prompt this call recorded with its text (a repository never recorded,
  or `store_prompts = false`, keeps its prompt), of 20 characters or more, and only to a viewer
  that holds its lock and says it listens on the configured port (`view::resident_answering`).
  The request names the repository; the viewer has no use for the branch.
- **The viewer** (`search::b::prompt_near`) embeds as a search does and answers keys and cosines
  only, the 20 nearest: delivered claims (under `DECIDED_WHERE` and with no link ending them, as
  `claims::delivered_one` reads them, and none an owner's change the worker has not applied yet
  touches, before anything is kept), none the session was shown (the viewer reads the session's
  stored shown set, which the hook names by its agent and session, whatever its size; a prompt
  that shows the manifest, which replaces that set, sends the manifest's keys instead), cards
  and summaries at `T` or above. The hook reads each through its reader and gates its line. No
  reranker or judge exists in the code; this route calls neither, and #336's, once built, stay
  out of it (spec 4.3). The route takes the viewer's token as every route does but no Origin: the
  hook is no browser. It reads as the page's search does (a GET with the token and no Origin), a
  page of another site cannot send the token header (a custom header needs a preflight, and
  OPTIONS is 405), and it leaves the page `--open` handed the browser in place.
- **Order.** Decisions first, as before (`shortlist::PLACES` units, from the shortlist or
  `delivered_ranked`, with the claims the viewer found), then cards and summaries: the words list
  (by the share of the prompt's trigrams their line holds) and the meaning list (by cosine) fused
  by reciprocal rank (`search::b::rrf_lists`), 5 places in all. A short prompt still gets the
  claims its words match, as before (`shortlist::MIN_GRAMS`). Each line is dated with the
  document's ID, which `get` takes: `- <date> <type> card <id>: <title>: <narrative>` (a
  summary's card, whose title is its narrative's first sentence, by its narrative once; the whole
  narrative since 2026-10-21, below), `- <date> session summary <id>: <request>`; the block is
  "Memory that may bear on this prompt".
- **Shown set.** A card's or a summary's entry carries `"kind"`, and the correction pass skips it
  (#506 item 1). The cards and summaries the session start's block showed join the set too, so a
  prompt does not show them again; a compaction's manifest replaces the set, as before. Grok's turn
  keeps each pick with its kind (a list of uids kept before reads as claims).
- **`T` = 0.67**, measured without model calls on 2026-10-14 with the local bge-m3 (`oboete
  embed-spike`, the production loader and gate): the 50 no-answer dev questions against every
  active claim of their repository in the dev home (5,306), the lowest cosine at which at most 1 of
  the 50 gets anything (1 does, at 0.68). Against the delivered claims alone (53) it would be 0.64,
  too few to stand for a store's cards and summaries. At 0.67, 22 of the 42 false-premise
  questions with claims in their repository get one, which the one evaluation judges.
- **The wait** defaulted to 1,000 ms (2,000 ms since 2026-10-21: the first prompt after a pause,
  below), measured on 2026-10-14 on this PC's WSL: a copy of the dev home (5,307 claims and 2,691
  cards embedded by the local bge-m3), the resident viewer warm, 47 questions of the dev pool sent
  as the hook sends them, with claude-mem's own processes running as they do. The viewer's answer:
  p50 70 ms, p95 88 ms, max 101 ms. The whole prompt hook: p50 86 ms, p95 103 ms, max 116 ms with
  it; p50 24 ms, p95 37 ms, max 71 ms with the words side alone; p50 19 ms, p95 24 ms on `main`
  before this change. A model the viewer dropped loads in about 2 s, the viewer answering at once
  without a vector meanwhile. The default leaves ten times the 95th percentile, for a busier machine
  and a larger store (the bit index's scan grows with it), and stays under claude-mem's 2 s for
  Codex.
- **Doctor and the page.** Doctor's per-prompt line says whether a resident viewer answers now, and
  how the last prompt that could ask took its block (`state/prompt-side`: in time, after the wait,
  a request the viewer failed before the wait was over, no viewer, or the viewer's reason for no
  vector); the settings page shows the same under the wait, in both languages.
- **The reader's model.** In a home that embeds with the local model and has its files, the
  resident viewer loads its model, or keeps it, whenever the hooks record again (it looks every
  2 s for a newer hook record), and at its own start when the newest hook record is less than 30
  minutes old, as the session start that started it; it drops the model after 30 minutes with
  neither a search nor a hook record (`WARM_IDLE`), whichever request loaded it; a load takes
  about 2 s, past `QUERY_TIMEOUT`. So a session's start loads it before the first prompt, and the
  agent's work keeps it; a prompt after 30 minutes with nothing recorded still takes words alone
  and starts the load. Until 2026-10-17 only searches counted, for 10 minutes: of the 16 prompts
  of the week before, 15 came to a dropped model, where 13 came within 30 minutes of the hooks'
  last record (parity row 31). An MCP server or a CLI command that searches keeps its own model 10
  minutes after its last search (`READER_IDLE`), as before. The model holds about 2.5 GB while
  loaded.

## As built (the second slice: imported history, #506)

#506 asks that an imported document reach a checkout's block only through an explicit mapping to
the checkout's repository, never because the checkout's last path component is the project's name;
a document it cannot map stays search-only.

- **The mapping is the session.** The import keeps each document's session, the agent's own id
  (`content_session_id` for claude-mem: `import::Sink::put`). A document maps to the repository
  its session started in: the repository of oboete's first record of that session (the earliest by
  time, then device and seq, with a repository), whatever recorded it, a hook or an import of
  transcripts or of v1. A session oboete never recorded maps nowhere, and its documents stay
  search-only. No project name is read, so `a/payments` and `b/payments` each get the history of
  the sessions that started in them, and a project's sessions that started elsewhere go where they
  started (decision 52 adds the checkout's claude-mem names to this rule, below). v1's documents, which the migration files under a repository, map through their
  sessions too. An agent's note (spec 4.12), which has no session of an agent's, maps to the
  repository it was saved in. Measured on this PC's store on 2026-10-15: of its 248,608 imported
  documents that are not prompts, 153,544 (62%) have a session oboete recorded (9,067 of v1's
  9,069), and every recorded session's first record has a repository.
- **Where it is kept.** knowledge.db's `session_start(session, repo, ts, device, seq)`: the Fts
  consumer, which keeps `raw_docs` (a row per live record, with its session and repository), keeps
  each session's first row there as it reads records, an older record read later included, and
  works it out again from `raw_docs` for a session whose first record a forget or a rewind takes
  away. A store from before this slice fills it from `raw_docs` once, with the indexes
  `raw_docs(session, ts, device, seq)` and `imported(session, repo)`, made by the first process
  that opens the store with this version: on this PC's store (575,931 records, 5,787 sessions) the
  fill takes 0.5 s and the indexes 2.3 s and 3.5 s. Until the consumers have made the table, a
  prompt gets no imported history.
- **Which documents.** Imported observations and session summaries, and agents' notes; not
  imported prompts (a prompt's text is never injected, 4.6, and claude-mem's per-prompt search
  reads observations only), and nothing forgotten.
- **By meaning only.** The viewer (`search::b::prompt_near`) searches the imported vectors'
  knowledge partition (`k`, which holds no prompt), kept inside the KNN to the sessions that map to
  `repo` (and `NOTE_SESSION` when a note was saved there), so a project's documents of other
  sessions take none of its candidates: on this PC's store, 65 repositories have imported history
  mapped to them, and in 45 of them it is under 1% of what their projects hold. The bit index
  checks a session list row by row, about 1.2 ms a session at 248,608 vectors against about 110 ms
  for a scan by project, so a repository past `SESSION_FILTER` (128) sessions is searched by the
  projects its documents are filed under (`claude-mem:<project>`) instead: on this PC's store only
  this repository is, with 192 sessions whose documents are 99.7% of its projects'. Either way a
  hit at `T` is kept only when its newest row maps to `repo` (`consumer::imported::maps`) and its
  reader shows it (`consumer::imported::prompt_line`). The hook's words side does not read imported
  history: on this PC's store an English prompt's trigrams match nearly every imported document
  (about 250,000 of 263,207), and ranking them took 0.6 to 1.0 s for prompts of 48 to 141
  characters, which the prompt hook cannot spend; keeping only the prompt's rarest trigrams, one of
  which every document over the threshold must hold, still left 40,000 to 50,000 candidates to
  read. claude-mem's own per-prompt search is by meaning only. So imported history and notes come
  only while the resident viewer answers with a vector.
- **The line**, labelled imported with its status unknown (spec 4.5): `- <date> imported <kind>
  (status unknown) <uid>: <title>: <body>`, an observation's whole body (its narrative, then its
  facts) since 2026-10-21 (below), a summary by its body's first sentence (its request); an
  agent's note as `- <date> note <uid>: <title>: <text>`, the longer alone when one starts with all
  of the other (a note saved without a title is titled with its text's start).
  The uid is the one `get` takes. It joins the shown set with the kind `imported` (the correction
  pass skips it, as a card) and Grok's kept picks with that kind. The block's lead names notes and
  imported history and says imported history's status is unknown.
- **Order.** Claims first, as before; then cards, summaries and imported documents in the same
  fused lists, 5 places in all.
- **Search is unchanged.** Its scope by the project's name (`import::imported_repos`,
  docs/claude-mem-import.md I5) stays: search and timeline find a repository's imported documents
  by name, and only the per-prompt block asks for the session.
- **The viewer's answer** with imported history, measured on 2026-10-15 on a copy of this PC's
  store with the resident viewer warm, 25 invented prompts in English and Japanese asked twice from
  a checkout of this repository, whose sessions map 61,761 imported documents. As the store stood,
  with 20,666 of its 248,608 imported vectors made (the local model was still making the rest): the
  viewer's answer p50 90 ms, p95 146 ms, max 263 ms (52, 90 and 125 ms on `main` 873b7c9), and the
  whole prompt hook p50 103 ms, p95 119 ms (58 and 74 ms on `main`); 18 of the 50 prompts got a
  block, with 46 imported lines. With the imported partition filled to its full size by random
  vectors, for the time alone: the viewer's answer p50 167 ms, p95 239 ms, max 396 ms, and the hook
  p50 180 ms, p95 232 ms. The viewer's answers and the hooks were timed in separate runs, the
  answers from a client that opens a connection for each. The bit index's scan of the imported
  partition takes about 110 ms at that size whatever the repository filter (sqlite-vec reads every
  row of a partition), so a repository to which no imported document and no note maps skips it.
  This repository is searched by its projects; one of fewer sessions by its session list, which
  costs about 1.2 ms a session. The wait's default of 1,000 ms stayed: four times the 95th
  percentile at full size, and the imported history grows only by a later import (2,000 ms since
  2026-10-21, below).

## Decision 52: imported history by the project's name too (2026-10-20)

Spec decision 52 widens #506's rule: an imported claude-mem document also maps to a checkout when
its project is one of the names claude-mem gives that checkout, as claude-mem's own reads match
rows. The session rule stays, and so do v1's documents and agents' notes.

- **What claude-mem does (14.0.1).** Its file context and its context reads ask for
  `getProjectContext(cwd).allProjects` and take a row whose `project`, or whose
  `merged_into_project`, is one of those names, compared without case
  (`src/services/sqlite/project-read-keys.ts` `projectScopeSql`). Its default naming
  (`CLAUDE_MEM_PROJECT_NAME_SOURCE` unset or `path`, `src/utils/project-name.ts`
  `getPathProjectContext` and `src/utils/worktree.ts` `detectWorktree`) gives:
  - inside git, the name of the working tree's top level;
  - for a linked worktree (its `.git` a file whose `gitdir` ends in `.git/worktrees/<name>`), its
    main repository's folder name and `<main>/<worktree>`, or the main name alone when the two are
    the same, with `<main>/<main>` kept as a second name;
  - for a submodule (`gitdir` under `.git/modules/`), its superproject's folder name and
    `<super>/<its path under the superproject>`, with the submodule's own folder name as a third
    name when that differs from the superproject's;
  - outside git, the nearest ancestor holding `.claude-mem-project` or `.claude-mem.json`, the walk
    stopping before the home directory, the temporary directory or Claude's config directory (and
    never starting inside the last), with the directory's own name as a second name when it is
    below that root;
  - otherwise the directory's own name, and `unknown-project` at the filesystem's root.
- **What oboete copies.** `import::names_of(cwd)` gives those names for a hook's `cwd`, from
  files alone (`repo::top`, the `.git` file's `gitdir`, read as claude-mem reads it: one line,
  resolved against the checkout without following links). The project oboete keeps for an
  imported document (its `repo`, `claude-mem:<project>`) is already the one claude-mem merged it
  into (docs/claude-mem-import.md I3), so its comparison covers claude-mem's `merged_into_project`
  column, and its `project` column for the rows claude-mem did not merge: a checkout named after
  the project a merge took rows from does not get them, while a worktree gets the rows merged into
  its main project, whose name its names hold. A document maps to the checkout when its newest row's session started in the checkout's
  repository (#506), or its newest row's `repo` is `claude-mem:<name>` for one of the checkout's
  names, without case (SQLite's `NOCASE`, as claude-mem's).
- **Where.** The note on a file (docs/file-note.md F13) and this block. Search and the timeline
  are unchanged (they find a repository's imported documents by name already, I5), and session
  start shows no imported history (milestone 4 D6).
- **This block.** The hook sends the checkout's names with the prompt (`projects` in
  `POST /api/prompt-context`, at most 16, each 1 to 4,096 bytes, `import::NAME_BYTES`, a path's
  length, as a submodule's or a worktree's name joins its parent's; the hook leaves out a longer
  one, which the viewer would refuse with the whole request; a viewer from before this version
  refuses the field, and the hook has the words side alone until the viewer has moved to the new
  binary, R9). The names are read only when the viewer is asked. The viewer keeps the imported partition's KNN to the sessions that map to the
  repository and, now, the sessions of the documents filed under one of its names, while they are
  at most `SESSION_FILTER` (128); past it, to the projects, now with the names' (as the vectors
  file them, `embed_phase::vec_repo`: a worktree's documents under its main project). Either way
  `consumer::imported::maps` keeps each hit to the mapping. Past `SESSION_FILTER` the scan by a
  main project holds its other worktrees' documents too, which `maps` refuses for a checkout whose
  names leave them out; they can take some of the KNN's 400 candidates, as #506's scan lets a
  project's other sessions take some. The names' sessions and projects are
  read from a new index, `imported(repo COLLATE NOCASE, session, kind)`, which the imported
  consumer makes; a name's sessions are those of each document's newest row, as `maps` reads it,
  so a document moved since to another project brings its session no more.
- **Exclusions** (spec 5.5, row 30-2). An exclusion of a claude-mem project one of the names
  names, without case, keeps the prompt from a remote embedder, at the viewer's first check and
  at its check just before the call, as an exclusion of the project the repository's last part
  names (D13) always did; the local model's vector leaves no machine and is kept, as before.
- **Two checkouts with one name** (`a/payments`, `b/payments`) both get the history filed under
  `payments`, as in claude-mem: the cost decision 52 accepts.
- **Differences from claude-mem.** The names come from the hook's `cwd`, where claude-mem's
  Claude Code hooks take `CLAUDE_PROJECT_DIR` first when it is set: oboete's repository, cards and
  session rule all come from `cwd`, so its names do too. They come from the `.git` files, not from
  git's own discovery: in a subfolder of a checkout git refuses (a worktree made by Windows git and
  read from WSL, one whose main repository moved, another user's checkout), claude-mem names the
  subfolder by itself, oboete by the checkout; at the checkout's top both name it the same. claude-mem's optional naming
  (`CLAUDE_MEM_PROJECT_NAME_SOURCE=git-remote`, `CLAUDE_MEM_PROJECT_ENVIRONMENTS`) is not read; a
  history recorded under it maps by its sessions alone. A row claude-mem filed under one project
  and merged into another is matched by the project it was merged into, where claude-mem matches
  it by either; and the mapping follows claude-mem's merges as of the last plain import, which
  files a moved document again (docs/claude-mem-import.md I3): on this PC 358 of the 9,555 imported
  observations filed under a worktree's name (3.7%) were merged into another project by claude-mem
  after their import, and reach the main checkout only through their sessions until a plain
  import, which also brings what claude-mem recorded after the switch (claude-mem-parity.md, the
  first open check).

## The first prompt after a pause, and whole narratives (2026-10-21, parity row 31)

**What was wrong.** From 2026-10-13 to 2026-10-20 no prompt in Claude Code got oboete's block:
Claude Code's transcripts on this PC hold 779 additional contexts from prompt hooks, none of them
oboete's block and 31 claude-mem's (29 for typed prompts). The same prompts asked of the resident
viewer afterwards found documents for 11 of 15. A copy of this PC's store with its files dropped
from the page cache (`posix_fadvise`) showed what a cold store does: the viewer's answer took
3.65 to 3.92 s, past the hook's 1,000 ms (and past 3,000 ms, the longest wait the settings allow),
so such a prompt takes the words side, which nothing passed. That the owner's prompts met a cold
store is inferred, not measured: WSL runs in 22 GB with `autoMemoryReclaim = gradual` and swap in
use, and the two stores are 6.7 GB. The check is oboete's block counted in the transcripts once
this change is installed, and `state/prompt-side` after a prompt. Of the 3.9 s, `imported_sessions`
took 2.85 s: its side for decision 52's names read the row of each of the names' 54,946
documents to find whether it was its document's newest row. The KNN of the four families took
about 1.0 s more (claims 0.26 s, imported history 0.62 s), the prompt's vector 0.04 s. Warm, the
answer took 0.29 s and the hook 0.6 to 0.7 s.

**What changed:**

- **`IMPORTED_SESSIONS` looks for the newest row once a session**, not once a document: the
  names' sessions come from the index `imported_project`, and each is kept once one of its
  documents under the names is its document's newest row. The same sessions (180 for the name
  of this repository's checkout, 140 for another): 69 ms from a cold cache instead of 3,008, 10 ms
  warm instead of 106. The viewer's cold answer took 1.27 s with it.
- **`per_prompt_wait_ms` is 2,000 by default** (it was 1,000): a cold answer of 1.27 s then
  reaches the prompt. On the copy, cold, the prompt hook took 1.31 to 1.35 s and its block had
  five lines in each of 3 runs; at 1,000 ms it had none in 3 runs (1.22 to 1.24 s: cold, the
  hook's own words side takes about that long). 2,000 ms is claude-mem's budget for Codex's
  prompt hook (claude-mem waits up to 10 s for Claude Code's); a prompt waits only as long as the
  viewer takes, 0.5 to 0.7 s warm. The 2026-10-14 measurement behind the old default was on
  a copy of the dev home with a warm cache.
- **The line** shows a card's whole narrative after its title, as claude-mem's block shows an
  observation's (`/api/context/semantic` in `SearchRoutes.ts`; `CLAUDE_MEM_SEMANTIC_INJECT_FORMAT`
  is `narrative` by default in 14.0.1, and its hook passes the block on whole). A card stored
  without a title, titled with its narrative's first sentence, shows its narrative once
  (`cards::begins_with_title`: the text holds all of the title, to a word's or a sentence's end);
  a title that only begins a word of the narrative ("Plan" and "Planet-scale") is kept. An
  imported observation shows its whole body as stored, its narrative and then its facts
  (`import::observation_body`), as no mark tells the facts apart from a list in the narrative;
  an agent's note shows its whole text; a session summary, oboete's or imported, its request as
  before. A line is still cut at `consumer::manifest::CLIP` (400 characters) at a space or a
  clause mark, as a session start's line is, and `get` shows the rest.
- **What a prompt is matched on does not change**: a card's title and its narrative's first
  sentence. A longer text would let more cards past the threshold of 4.6, which the one evaluation
  sets (decision 34). The meaning side reads each document's vector, as before.
- **`per_prompt_chars` is 2,500 by default** (it was 1,500): five lines of 400 characters with the
  longest labels (an imported line's date, kind, `(status unknown)` and uid: 83 characters) and
  the heading take 2,459; 1,500 holds three such lines. On this PC from 2026-10-13 to 2026-10-20,
  claude-mem's block was 2,053 characters at the median and 3,712 at the 90th percentile, with five
  observations each. oboete's cards' narratives are 192 characters at the median and 290 at the
  90th percentile (3,454 cards), and claude-mem's observations' 408 and 624 (82,033 in the last 30
  days). A smaller `per_prompt_chars` keeps fewer whole lines: the block leaves out the lines past
  it.

A cold cache has no seam in the tests: the measurement is a script on a copy of the store
(the store copied with SQLite's backup, its identity kept, a viewer of its own on another port,
the files dropped from the page cache before each run, the prompt hook run as Claude Code runs
it). The tests hold what can be held: the query's plan looks for the newest row by the session in
the index, the defaults, the lines and what they are matched on.

Tests: `search::b::tests::imported_history_reaches_a_prompt_by_the_checkouts_names_too` (the
names' sessions, a moved document's session left out, the plan),
`hook::tests::the_default_block_holds_five_whole_narratives`,
`a_cards_later_sentences_do_not_carry_it_past_the_threshold`,
`a_prompt_gets_the_cards_and_summaries_its_words_bear_on` (a card's whole narrative),
`a_card_the_session_start_showed_is_not_shown_again` (a summary's card by its narrative once),
`a_title_that_only_begins_a_word_of_the_narrative_is_kept`,
`cards::tests::a_text_begins_with_its_title_only_at_a_words_end`,
`consumer::imported::tests::a_prompt_names_an_imported_document_by_its_newest_row` (an
observation's whole body, a title that only begins a word of it kept, a note whole, a summary by
its request), and `config::tests::inject_settings_check_ranges_and_save_alone` (the defaults).
Mutation check: 13 mutants (the two defaults, a card's line, a summary card's title once, the
words matched, an imported observation by its first sentence or cut at its facts, a summary by
its whole body, the title check as a plain prefix, without the cut or without the sentence's end,
the query once a document again, the query without the newest row), each failed by one of these
tests.

## Where it does not reach yet

- The resident worker and viewer are Linux only for now (`view::resident_home`). On the iMac the
  block is the words side alone until resident mode is built there; doctor says so.
- Imported history whose session oboete never recorded (#506): search finds it, the block does
  not.
- Imported history and notes by words (the second slice): the block takes them by meaning only.
- A Workers AI embedding is made for each prompt that asks (the iMac, once resident): it counts
  in the embedding budget and its daily cap, as search queries do, and doctor shows the count.

## Decision 44

Spec decision 44: at prompt submit the hook asks the resident viewer (replacing decision 38's
"Not now" for it); imported history may appear in the per-prompt block (an exception to 4.5);
cards, summaries and imported history join claims, each above the threshold, and the block is
empty when nothing passes.

## Spec changes

With this note: decision 44, and pointers to it in 4.1, 4.5 and 4.6. With the code: 4.2 (the
prompt hook's steps), 4.6 (the two-sided threshold and `T`'s provisional calibration), the
`[inject] per_prompt_wait_ms` key, the parity table (row 31: per-prompt hints), and the settings
page's injection section (the wait, and which side answered last).
With the second slice: 4.2, 4.5, 4.6 and 4.12 say how imported history and notes reach the
block, and by meaning only.

## Tests

As built (the first slice). The three tests with a stand-in viewer run on Linux only, where the
resident viewer keeps its token (`keyfile::private_fs`, #281):

- The hook asks only a viewer that is up, with the checkout's repository; what it finds near in
  meaning joins the block; a claim it names that is not delivered stays out
  (`hook::tests::the_prompt_point_takes_the_resident_viewers_meaning_side`).
- An answer past the wait: the words side's block, within the wait
  (`a_late_answer_leaves_the_words_side_within_the_wait`); a request the viewer fails is told
  from a late one (`a_failed_answer_is_told_from_a_late_one`).
- A card or summary its reader hides takes no place, in the viewer's 20 nor in the words side's
  depth (`search::b::tests::a_document_no_reader_shows_takes_no_place`); nor a summary without a
  request (`a_summary_without_a_request_takes_no_place`), nor what the session was shown, which
  the viewer reads from the session's stored set, or a manifest shown at the prompt sends
  (`prompt_near_keeps_delivered_claims_and_near_documents`,
  `view::tests::the_prompt_context_route_reads_the_sessions_shown_set`,
  `hook::tests::the_prompt_point_takes_the_resident_viewers_meaning_side`,
  `a_prompt_that_shows_the_manifest_sends_its_keys`). On the words side, only a line at the
  threshold takes one of the depth's places (`a_card_short_of_the_threshold_takes_no_place_in_the_depth`).
- A prompt not recorded with its text, or under 20 characters, is sent nowhere
  (`a_prompt_not_recorded_whole_or_too_short_asks_nothing`).
- No resident viewer: the words side, over cards and summaries too, after the decisions, each line
  dated with its ID, joining the shown set with its kind (`a_prompt_gets_the_cards_and_summaries_its_words_bear_on`);
  the correction pass names neither (`a_card_shown_at_a_prompt_draws_no_correction`); what the
  session start showed is not shown again, and a summary's card shows its sentence once
  (`a_card_the_session_start_showed_is_not_shown_again`); a store without the documents' indexes
  still gets its decisions (`a_store_without_the_document_indexes_still_gets_its_decisions`).
- The viewer keeps delivered claims only, a decided lesson a later one ended among those left
  out, before anything is kept, and documents at `T`
  (`search::b::tests::prompt_near_keeps_delivered_claims_and_near_documents`); an excluded
  repository's prompt goes to no remote embedder (`prompt_near_sends_no_prompt_of_an_excluded_repository`);
  the route takes the token and no Origin and leaves the opener page
  (`view::tests::the_prompt_context_route_takes_the_token_but_no_origin`).
- The wait on the settings page and in config.toml, kept when an older page omits it
  (`settings::tests::inject_settings_check_ranges_and_save_alone`).
- The measurements above: `T` from the no-answer questions; the viewer's answer and the prompt
  hook's time on this PC's WSL.

The second slice:

- With no names given, imported history reaches a prompt only through its session (#506's
  acceptance test, which decision 52 widens with the names): history
  imported from a project named `payments` reaches neither `a/payments` nor `b/payments` while
  oboete recorded none of its sessions; once one session started in each, each checkout gets that
  session's documents only, and a session never recorded goes to neither, a v1 document filed under
  the checkout's repository included; an agent's note reaches the repository it was saved in; a
  prompt, a forgotten document and one the session was shown take no place; a store without
  `session_start` gets no imported history
  (`search::b::tests::imported_history_reaches_a_prompt_only_through_its_session`).
- 401 documents of a session never recorded, nearer the prompt than the repository's own one and
  more than the KNN's 400 candidates, do not crowd it out
  (`search::b::tests::a_repositorys_imported_history_is_not_crowded_out_by_other_sessions`); past
  128 sessions the projects' scan still keeps to the mapping
  (`a_repository_of_many_sessions_keeps_to_its_mapping`).
- `session_start` keeps each session's first record as records come, an older one read later, a
  rewind and a forget, and fills a store from before it; each record's text keeps its own row in
  the raw index (`search::tests::session_start_keeps_where_each_session_started`).
- The line: an observation by its title and its whole body, a summary by its request, a note
  saved without a title by its whole text once, none for a prompt or a forgotten document
  (`consumer::imported::tests::a_prompt_names_an_imported_document_by_its_newest_row`).
- In the hook: the viewer's imported hit joins the block with its line, the shown set with its
  kind, is not shown again nor corrected, and Grok's turn keeps it
  (`hook::tests::imported_history_the_viewer_finds_joins_the_block`).
- Mutation check: 16 mutants of the mapping, the table's upkeep, the reader, the KNN's filter,
  the scan's skip and the hook's kind, each failed by one of these tests.

Decision 52:

- The names: a checkout and a folder in it (the top level's name), a linked worktree (the main
  repository's name, then `<main>/<worktree>`; one named as its main repository, `<main>/<main>`
  then the main name), a submodule by a relative `gitdir` (the superproject's name, its own, then
  `<super>/<path>`), a `.git` file of more than its line or naming neither (a checkout of its
  own), a folder below a marker root (its own name, then the root's) and the root itself, no
  marker (the folder's name), a path with `..` folded first (a link before it folded away with it,
  not followed), a walk that stops before a stop and
  never starts in the config directory, and the filesystem's root (`unknown-project`)
  (`import::tests::a_checkouts_names_are_claude_mems_by_path`).
- This block: with a worktree's two names, the first in other letters' case, a document of a
  session never recorded filed under the main project or the worktree's reaches the prompt; one
  of another worktree, of another name or a v1 document of a session never recorded does not;
  without names nothing does; a document filed under a name and moved since brings its session
  no more; past `SESSION_FILTER` sessions the projects' scan finds a near document among them;
  the names' sessions are read through `imported_project`
  (`search::b::tests::imported_history_reaches_a_prompt_by_the_checkouts_names_too`).
- A checkout one of whose names, in any case or as a worktree's, an excluded claude-mem project
  names has its prompt embedded by no remote provider, and another name's is
  (`search::b::tests::prompt_near_sends_no_prompt_of_a_checkout_an_excluded_project_names`).
- The viewer takes `projects`, at most 16 names of 1 to 4,096 bytes, and refuses more, an empty
  one, a longer one or a string (`view::tests::the_prompt_context_route_takes_the_token_but_no_origin`);
  the hook sends the checkout's names (`hook::tests::imported_history_the_viewer_finds_joins_the_block`)
  and leaves out one past the limit (`a_name_past_the_viewers_limit_is_not_sent`).
- The note on a file: docs/file-note.md test 25
  (`hook::tests::imported_history_filed_under_the_checkouts_name_joins_the_note`).
