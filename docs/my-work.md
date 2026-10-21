# My Work: what was asked, and where it stands (parity row X10)

Plan of the slice that builds claude-mem 14.0's My Work view in oboete's page (decision 53,
2026-10-20; issue #570), written before the code, as the other slices' notes are. The behaviour
of record is claude-mem v14.0.1; every rule below names the file it comes from there.

## What claude-mem does (14.0.1)

My Work is the default page of claude-mem's worker (`/`); the earlier viewer moved to `/viewer`
(`src/services/worker/http/routes/WorkViewRoutes.ts`). It reads the memory database read-only and
makes no model call to show anything (`docs/public/usage/my-work.mdx`).

- **A request** is a conversation: the agent's platform and its session (`request-projection.mjs`,
  `conversationKey`), made of the work tied to it (below); a conversation with none is not shown.
  Its original ask is the session's first prompt that reads as a person's request for action
  (`isHumanActionRequest`: envelopes, greetings and acknowledgements such as "ok", "continue" or
  "resume" are not; a prompt with no English action word in a list of about 45, "fix", "add", "can
  you" and the like, is not either); its later prompts are its follow-ups (scope updates, the newest
  6, 500 characters each). Its title is a work item's `human_request`, `request` or `user_request`
  when one is recorded and reads as a request, else the original ask's first sentence (100
  characters). A work-state field `request_id` names a request of its own: the work items that
  carry it, across conversations too, make one request, and a conversation's requests share its
  original ask and follow-ups. In a conversation of one such id, all of its work joins that
  request; in a conversation of several, work without an id is unlinked. Records with no proven
  human request are counted apart: "N work records need a linked human request".
- **Its work** is what `work_state_write` recorded (lists and tasks with `status`), each entry tied to
  a session only through the tool call that wrote it: same project, list and fields, within 5 s,
  and only when exactly one call matches (`agent-view.mjs`). Each prompt with observations is a
  work record of its own, with that prompt's session summary's `next_steps` and `completed` as its
  next step and outcome; an observation is the activity of the request whose work was written, or
  recorded, in the same prompt. Bounds: 240 work items, 300 observations, 100 summaries, the
  prompts of the loaded sessions (1,200 characters each).
- **Statuses** (`view-model.js` `statusFor` and `buildModel`, `presentation.js`):
  - The request's own recorded status decides first: its completion (`request_complete: true`, or
    a `request_status` of `done`, `completed`, `complete`, `shipped` or `delivered`) is "Ready to
    check", and another status a `request_status` names (`blocked`, `needs-you`, `closed` and the
    like) is that status.
  - Otherwise its work decides, the first that holds: "Needs you", a recorded status `needs-you`,
    `needs-input`, `waiting-on-user`, `awaiting-approval` or `needs-approval` (lower-cased, `_` and
    spaces read as `-`); "Blocked", `blocked` or `failed`; "Update recorded", all of it done or all
    of it dropped (a finished task or an ended session does not finish its request); "No recent
    update", nothing newer recorded for 6 hours; "In progress", `doing`, `in-progress`, `active` or
    `running`; "To do", `todo`, `planned` or `queued`; else "Update recorded".
  - "Checked": a person marked this version of the result checked; it shows in place of the
    status.
  - The status tabs always show All, Needs you, In progress, No recent update, Ready to check and
    Checked, and Blocked, To do, Update recorded and Closed when they have a count. "Attention
    first" orders needs you, blocked, ready, working, quiet, todo, unknown, closed, checked.
  No prompt in claude-mem asks an agent to write `request_complete` or `request_status`: an agent
  sets them only if it chooses to.
- **The card**: its status, the time of its last update, a title, a two-line summary, who worked on
  it (by agent), its projects, and a next step or outcome with "Review result", "Read handoff" or
  "Open request". **Views**: cards (default), a list (status, request, next step or outcome, last
  update) and by agent. **Filters**: search (title, activity, agent), project, time range (past 24
  hours by default, past 7 days, all recorded work), sort (attention first by default, latest,
  oldest, A to Z) and status tabs with counts. **The drill-down**: the original ask, the follow-ups,
  the result, each work stream's recorded changes (up to 100) with their source records, and "Copy
  handoff" (the goal, the recorded state, the next step and recent updates, as text to paste into
  an agent).
- **Model-written copy** (`human-copy.mjs`, `human-copy-worker.mjs`): a title (100 characters), a
  summary (240), a next step (160), a stage (40) and each work stream's title and summary, written
  per request from at most 18,000 characters of its records, again when they change (once they have
  been quiet for 6 s, or 20 s after the first change, at most once a minute a request), the card
  showing "Summary updating" meanwhile. It runs only when `CLAUDE_MEM_PROVIDER` is `openrouter`
  with an OpenRouter key (`WorkViewRoutes.ts`); the default provider (`claude`) writes none, and
  the owner's claude-mem runs with `claude`, so it has written none. Without it (`presentation.js`)
  a card shows the recorded wording: the request's title above, the newest recorded update (a work
  item's progress or outcome, an observation's subtitle) as its summary, else the original ask, and
  the newest recorded next step (a work item's `next_step`, a prompt's `next_steps`), else a line
  for its status ("Follow the latest recorded activity.").
- **Checked** (`review-store.ts`): a separate database, `work-view-reviews.db`, one row per request
  (its id, a SHA-256 of the request's recorded content, the time, `local-user`), never in the memory
  database and never synced. The content is the original ask, the follow-ups, the title, the
  projects, the agents, the work items and the recorded completion; the generated copy is left out,
  so a new summary does not undo a check (`contentFingerprint`). A newer recorded version needs a
  new check; Undo removes the row. `POST /api/work-view/review` takes the version it checks and
  refuses with 409 a stale one, and a check of a request not reported complete.
- **Updates**: the page gets a new snapshot when the database changes (server-sent events, polled
  each second on the worker side); while a person reads, scrolls or uses a control, new data waits
  behind "Show updates", and applied cards keep their order.
- Not part of the parity: TeamBrain (paid team sharing) and the "Connect Claude.ai / ChatGPT" links
  to claude-mem's cloud.

## What oboete has

- The prompts, as records of kind `prompt` with their agent, session and sequence (raw.db).
- One session summary per reply (`turns`): `request`, `investigated`, `learned`, `completed`,
  `next_steps`, `notes`, written by the curators in their order (docs/summaries.md), read through
  `turns::get` and `turns::page`, which hide what a forget or a removal took.
- Cards (observations) per window, read through `cards::get` and `cards::of_turn`.
- Work state (docs/work-state.md): the same lists and tasks as claude-mem's, written by
  `work_state_write` as ops in raw.db with no session of their own; the agent's tool call that
  wrote one is a record of kind `tool` in its session.
- The page (`src/view.rs`, `assets/viewer/app.js`): tabs (Timeline, Records, Context, Stats,
  Settings), JSON routes behind the token, writes behind `save_gate` (Host, token, Origin, JSON,
  a length cap), every string through the outbound gate, Japanese and English labels (#94), a poll
  every 3 s (parity row 49).

## The design

1. **A request** is an agent's session, as claude-mem's conversation: `(agent, session)`, or an
   explicit `request_id` in work state, by claude-mem's rules above: the requests of one
   conversation share its original ask and follow-ups, and differ in their title and their work. The
   original ask is the session's first prompt with its text recorded (1,200 characters) that is not
   an envelope, a greeting or an acknowledgement, in English or Japanese ("ok", "continue",
   "resume", "はい", "続けて", "再開して" and the like); the follow-ups its later prompts (the newest 6, 500
   characters each). Unlike claude-mem, a prompt needs no English action word: most of the owner's
   prompts are Japanese, and claude-mem's list of words would leave them out. A session none of
   whose prompts asks, one resumed with "再開して", takes its oldest reply's `request`, which the
   curator wrote, as its ask, and all of its prompts follow it up (added with slice 1: of the six
   live Claude sessions on this PC on 2026-10-21 one was so, which claude-mem shows under no
   request, its work counted among the unlinked). A prompt kept private, a session in an excluded
   repository and a forgotten record take no part, as everywhere. A request owns the replies (turns)
   of its conversations, with their summaries and cards; in a conversation of several `request_id`s,
   each owns only the replies in which its own work items were written (the reply of the linked
   `work_state_write` call), as claude-mem gives such a request only the observations of the prompts
   that wrote its work, and each reply that none owns, with cards, counts once among the unlinked
   records (claude-mem counts each of the conversation's prompts with observations). A request of
   several conversations takes the earliest of their original asks (claude-mem takes the first it
   reads) and the newest 6 of their follow-ups.
2. **Its work**: each work-state entry is tied to the session whose `tool` record of
   `work_state_write` names the same list and the same fields within 5 s of the op, and only when
   exactly one does (claude-mem's rule). The rest is the unlinked count. Only the work state this
   home's agents wrote is read: the work state imported from claude-mem (sourced ops, row N1) has no
   tool call in a session and stays out with the rest of the imported history (8). The cards of the
   replies it owns (1) are its activity, the newest first.
3. **The copy is the session summaries**, so My Work adds no model call. The card's title is a
   work item's recorded request when one reads as a request (claude-mem's rule, read as in 1), else
   the first reply's `request` (a short title the curator wrote, claude-mem's own field), cut to 100
   characters at a word or a clause; its summary the latest reply's `completed`, else `learned`,
   else `investigated`, cut to 240; its next step the latest `next_steps`, cut to 160; the replies
   are those the request owns (1). A reply not summarized yet keeps the last summary shown, as
   claude-mem keeps its last copy while a new one is written, and marks the card "Summary
   updating". A request with no summary yet (no curator configured, none answering, or not curated
   yet) shows its recorded wording, as claude-mem's cards do without their copy (decision 53): the
   recorded request, else the original ask's first sentence, as the title; the newest recorded
   update (a work item's progress or outcome, a card's subtitle), else the original ask, as the
   summary; the newest recorded next step (a work item's `next_step`), else a line for its status.
   There is no stage and no per-stream copy, which claude-mem writes only with a model either.
   Issue #570's measurement before turning summaries on is then zero added calls; that is stated,
   not assumed: the slice's test counts the provider calls a My Work read makes (none).
4. **Statuses** are claude-mem's rules above, from the request's recorded status and its linked
   work state; a request with no linked work is "Update recorded", and "No recent update" after 6
   quiet hours, as claude-mem's records of observations are. Attention first sorts as claude-mem's.
5. **Bounds**: the requests of the time range (24 hours by default), at most 240 linked work items,
   300 cards and 100 summaries read, as claude-mem's; the readers page by time and stop there.
6. **Checked**: `state/my-work-reviews.json`, one entry per request id: a SHA-256 over the request's
   recorded content (its original ask, follow-ups, projects, agents, linked work items with their
   fields and statuses, and its recorded completion, in a fixed order), the time, `local-user`. The
   curators' summaries, and a title taken from them, are left out, as claude-mem leaves out its
   generated copy: a new summary does not undo a check, a new follow-up does (for each request of
   its conversation, as in claude-mem). It is the page's file, not memory: not in raw.db, not in a
   backup or a sync, not searched. A forget that takes a session's records (`oboete forget`) drops
   that session's entries when it commits. `POST /api/my-work/review` (through `save_gate`) takes
   `{request, version, checked}`; a version that is not the current one, or a check of a request
   not reported complete, is refused with 409, and the page reloads the request. Undo is the same
   route with `checked: false`.
7. **The page**: a My Work tab, first and opened by default, as claude-mem opens on My Work; the
   Timeline is one tab away. Cards, list and by agent; search, project, time range, sort and the
   status tabs with counts; the drill-down with the original ask, the follow-ups, the summaries
   (the result), the linked work's changes and the cards, each named by the ID `get` takes, and
   "Copy handoff". New data waits behind "Show updates" while a person reads, scrolls or uses a
   control, and applied cards keep their order. Labels in Japanese and English.
8. **Imported claude-mem history** stays out: its sessions end before the switch (2026-10-15), past
   claude-mem's own default ranges, and claude-mem's My Work shows them where they were written.

## Slices

1. **The reader and its route**: `GET /api/my-work` (requests of a range, their status, copy, work
   and counts, gated), the unlinked count, and the tests of 1 to 5.
2. **The page**: the tab, the three views, the filters, the drill-down, Copy handoff and the
   reading-safe updates.
3. **Checked**: the file, the route, Undo, the stale version, and the forget path.

## Tests (acceptance, from #570 and the rules above)

- A session's first prompt that is not an envelope, a greeting or an acknowledgement (English or
  Japanese) is its request's original ask, its later prompts the follow-ups (the newest 6); a
  private prompt, an envelope, an excluded repository and a forgotten record are not shown.
- Two `request_id`s in a session make two requests with the same original ask and follow-ups, each
  titled by its own recorded request, each with the summaries and cards of the replies that wrote
  its work; a reply of neither, with cards, counts once among the unlinked records. One id in a
  session takes all of it.
- Work state ties to the session whose single matching `work_state_write` call is within 5 s; two
  matching calls, or none, tie it to no session, and it is unlinked and counted unless it names a
  `request_id` with a recorded request, which is then a request of its own on that wording, as
  claude-mem's `explicit-work-state-request`; while a session is excluded such work is neither
  shown nor counted, since it may be that session's; work state imported from claude-mem is
  neither read nor counted.
- Each status and alias above, from the request's recorded status and its work state; work that is
  all done, or an ended session, alone is not "Ready to check"; 6 quiet hours is "No recent
  update"; attention first orders them.
- The card's title, summary and next step come from the session summaries in their order, cut at a
  word; a newer reply not summarized keeps the last summary and says "Summary updating"; a request
  with none shows its recorded wording.
- A My Work read makes no provider call.
- A check is kept for its version only; a newer version asks again, a new summary does not; Undo
  removes it; a stale version and a request not reported complete are refused; a forget of the
  session's records removes its entries.
- TeamBrain is not part of it.

## Not built

- Model-written per-request copy, stages and per-stream copy (claude-mem writes them only with
  OpenRouter as its provider; the owner's writes none).
- TeamBrain and claude-mem's cloud links.
- Server-sent events: the page polls, as it does for its other views (row 49).

## As built

### Slice 1: the reader and its route (2026-10-21)

- `src/my_work.rs` reads the requests, and `GET /api/my-work?range=day|week|all` answers with them:
  a day by default, another value refused with 400 before anything is read, the newest update
  first, with the unlinked count.
- **What is read**: the newest 2,000 live prompts, 100 session summaries (`turns::page`) and 300
  cards (`cards::page`) of the sessions no exclusion takes, and this store's own work state writes
  (`Raw::native_work_writes`), folded as claude-mem folds them (each list's tasks, and the list
  itself when its state has a key besides `task`), of which the newest 240 that no excluded
  repository or session wrote are read: what is excluded takes no part in a bound, so it pushes out
  no older record (Codex on #574). Each reader pages on past an excluded session's prompts,
  summaries and cards, and past a card of no session (its window spans two sessions' records, so
  it is no request's), until its bound holds; an excluded session's items are known only once
  linked, so they are linked and left out until 240 are kept.
  The 2,000 prompts are a shortcut: claude-mem reads every prompt of the sessions it loaded, so a
  session whose first prompt is older than them has its oldest read prompt as its ask.
- **The link**: a write is the work of the session whose call of oboete's `work_state_write`
  (named as `curate::served` and `curate::oboete` read a tool: `mcp__oboete__…`, `oboete__…`,
  `oboete.…`) sent the same repository, list and fields within 5 s, when exactly one did; an item
  is the work of its newest write that links, as claude-mem credits it. The calls are found by
  time through `raw_docs_tools`, a new index the fts consumer makes in knowledge.db, as it makes
  `raw_docs_prompts` (2.6 s once on this PC's store, and no hook waits on it); before the worker
  has made it, nothing links. An item no call ties to a session may be an excluded session's
  write in a shown repository (capture refuses that session's later tool calls), so while a
  session is excluded it is neither shown nor counted, in every repository, and before the index
  no work is (Codex on #574). On this PC each of the 77 writes had exactly one such call,
  14 to 158 ms after it. Each tool record is read once in a read, however many writes it is near.
  A write whose fields the gate stored otherwise than its call's input links to nothing.
- **The request's own status** (#570): the newest record among its items of `request_complete` or
  `request_status`, each from the item's own fields, else its list's, `request_complete` first at
  the same time, decides first; then its work's, claude-mem's `statusFor`. A recorded
  `request_status` reads with the aliases an item's status reads with (`failed` is blocked,
  `running` in progress, `queued` to do, `dropped` closed), where claude-mem's lifecycle takes only
  its exact status names and leaves the rest to the work (Codex on #574). A request's prompts,
  replies and cards are one more row of its work, as claude-mem's observations are, so work that
  is all done reads "Update recorded" until it has been quiet for 6 hours.
- **Its identity** (#570): `c:<agent>:<session>`, or `r:<request_id>`, the id trimmed and a number
  written as its text, so `7` and `"7"` are one request. A session is known by its labels as the
  stores hold them, and the gate only shows them in the answer, so two sessions that a rule masks
  alike stay two requests and an excluded session's mask hides no other session (Codex on #574).
- **Status names** in the answer: `needs`, `blocked`, `ready`, `quiet`, `working`, `todo`,
  `unknown`, `closed`: the names claude-mem's page gives `needs-you`, `done` and `in-progress`, and
  its own for the rest. Checked comes with slice 3.
- **"Summary updating"**: the request's newest prompt has no turn op yet, kept or skipped, that
  covers it.
- **The recorded wording**, before any summary: the newest of its work items' outcome or progress
  and its cards' subtitles, else its ask, is the summary, as claude-mem's newest recorded update.
- **The unlinked count**: the work items no request takes; and, of a conversation with no request
  or with several `request_id`s, each reply no request owns that has a card, and the cards of no
  reply yet, once.
- **The gate**: an ask is told from an acknowledgement on the prompt's own text, and only what is
  shown goes through the gate, as the rules are now: an ask's first 1,200 characters, or a
  follow-up's first 500, and 4,000 more, so that a value the cut splits is whole when the rules
  read it, a key block the cut leaves open left out, as `hook::clip` keeps a field. The live
  prompts on this PC are 226,000 characters (39 prompts, most of them delegated reviews), which the
  gate read whole in 1.1 s. Work state's names and values go out as its own lines show them, each
  beside its key (`work_state::beside`); an item's other fields are not in the answer.
- **Speed**, on a copy of this PC's store (10 requests in a day, 18 in all): 2.4 s cold and 1.0 s
  warm, most of it the gate the summaries' and the cards' readers put every field through. The
  page reads it only when the store's version moves (row 49), and each connection to the viewer
  has a thread of its own, so a hook's request does not wait on it.
- **The answer** of a request: `id`, `title`, `summary`, `next`, `copy` (`summary` or `recorded`),
  `updating`, `status`, `updated`, `started`, `agents`, `projects`, `repos`, `ask` (`id`, `ts`,
  `text`, and `from`: `prompt`, `summary` or `work`), `follow_ups` (each `id`, `ts`, `text`), `work`
  (each item's `list`, `task`, `status`, `updated`), and `summaries` and `cards`, the IDs `get`
  takes, the newest first, at most 40 cards.
