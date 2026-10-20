# My Work: what was asked, and where it stands (parity row X10)

Plan of the slice that builds claude-mem 14.0's My Work view in oboete's page (decision 53,
2026-10-20; issue #570), written before the code, as the other slices' notes are. The behaviour
of record is claude-mem v14.0.1; every rule below names the file it comes from there.

## What claude-mem does (14.0.1)

My Work is the default page of claude-mem's worker (`/`); the earlier viewer moved to `/viewer`
(`src/services/worker/http/routes/WorkViewRoutes.ts`). It reads the memory database read-only and
makes no model call to show anything (`docs/public/usage/my-work.mdx`).

- **A request** is a conversation: the agent's platform and its session (`request-projection.mjs`,
  `conversationKey`). Its original ask is the session's first prompt; later prompts are its
  follow-ups (scope updates, the last 6 kept). A work-state field `request_id` can split one
  conversation into several requests, and a conversation with several such ids and an item without
  one leaves that item unlinked. Records with no proven human request are counted apart: "N work
  records need a linked human request".
- **Its work** is what `work_state_write` recorded (lists and tasks with `status`), each entry tied to
  a session only through the tool call that wrote it: same project, list and fields, within 5 s,
  and only when exactly one call matches (`agent-view.mjs`). The 300 newest observations and the
  100 newest session summaries add activity, a next step (`next_steps`) and an outcome
  (`completed`). Bounds: 240 work items, 300 observations, 100 summaries, the prompts of the loaded
  sessions (1,200 characters each).
- **Statuses** (`view-model.js` `statusFor`, `request-projection.mjs`):
  - Needs you: a recorded status `needs-you`, `needs-input`, `waiting-on-user`,
    `awaiting-approval` or `needs-approval` (lower-cased, `_` and spaces read as `-`).
  - Blocked: `blocked` or `failed`.
  - Ready to check: the request's own completion was recorded, `request_complete: true` or a
    `request_status` of `done`, `completed`, `complete`, `shipped` or `delivered`. A finished
    task or an ended session does not finish its request.
  - Checked: a person marked this version of the result checked.
  - In progress: a recorded `doing`.
  - No recent update: nothing newer recorded for 6 hours.
  - Others (todo, quiet, unknown, closed) keep their recorded label under "Other states".
  - "Attention first" orders needs you, blocked, ready, working, quiet, todo, unknown, closed,
    checked.
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
  showing "Summary updating" meanwhile. It runs only when
  `CLAUDE_MEM_PROVIDER` is `openrouter` with an OpenRouter key; the default provider (`claude`)
  writes none, and the cards show the recorded wording. The owner's claude-mem runs with `claude`,
  so it has written none.
- **Checked** (`review-store.ts`): a separate database, `work-view-reviews.db`, one row per request
  (its id, a SHA-256 of the request's recorded content, the time, `local-user`), never in the memory
  database and never synced. A newer recorded version needs a new check; Undo removes the row.
  `POST /api/work-view/review` takes the version it checks and refuses a stale one.
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

1. **A request** is an agent's session, as claude-mem's conversation: `(agent, session)`, split by an
   explicit `request_id` in work state as claude-mem splits one. The original ask is the session's
   first prompt with its text recorded (1,200 characters), the follow-ups its later prompts (the
   last 6). A prompt kept private, a harness envelope, a session in an excluded repository and a
   forgotten record take no part, as everywhere.
2. **Its work**: each work-state entry is tied to the session whose `tool` record of
   `work_state_write` names the same list and the same fields within 5 s of the op, and only when
   exactly one does (claude-mem's rule). The rest is the unlinked count. Cards of the session are its
   activity, the newest first.
3. **The copy is the session summaries**, so My Work adds no model call. The card's title is the
   first reply's `request` (a short title the curator wrote, claude-mem's own field), cut to 100
   characters at a word or a clause; its summary the latest reply's `completed`, else `learned`,
   else `investigated`, cut to 240; its next step the latest `next_steps`, cut to 160. A reply not
   summarized yet keeps the last summary shown, as claude-mem keeps its last copy while a new one is
   written, and marks the card "Summary updating". A session with no summary yet (no curator
   configured, none answering, or not curated yet) shows its recorded wording: the original ask's
   first line as the title, its last prompt as the summary (decision 53's fallback). There is no
   stage and no per-stream copy, which claude-mem writes only with a model either. Issue #570's
   measurement before turning summaries on is then zero added calls; that is stated, not assumed:
   the slice's test counts the provider calls a My Work read makes (none).
4. **Statuses** are claude-mem's rules above, word for word, from the linked work state; a session
   with no linked work is "unknown" with its last update time, so a session quiet for 6 hours is "No
   recent update" as claude-mem's are. Attention first sorts as claude-mem's.
5. **Bounds**: the requests of the time range (24 hours by default), at most 240 linked work items,
   300 cards and 100 summaries read, as claude-mem's; the readers page by time and stop there.
6. **Checked**: `state/my-work-reviews.json`, one entry per request id: a SHA-256 over the request's
   recorded content (its original ask, follow-ups, linked work items and their statuses, the
   summaries' fields, in a fixed order), the time, `local-user`. It is the page's file, not memory:
   not in raw.db, not in a backup or a sync, not searched; `oboete purge` and a forget of the
   session's records drop its entries. `POST /api/my-work/review` (through `save_gate`) takes
   `{request, version, checked}`; a version that is not the current one is refused with 409, and
   the page reloads the request. Undo is the same route with `checked: false`.
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
3. **Checked**: the file, the route, Undo, the stale version, and the purge and forget paths.

## Tests (acceptance, from #570 and the rules above)

- A session's first prompt is its request's original ask, its later prompts the follow-ups; a
  private prompt, an envelope, an excluded repository and a forgotten record are not shown.
- Work state ties to the session whose single matching `work_state_write` call is within 5 s; two
  matching calls, or none, leave it unlinked and counted.
- Each status and alias above, from recorded work state; a finished task or an ended session alone
  is not "Ready to check"; 6 quiet hours is "No recent update"; attention first orders them.
- The card's title, summary and next step come from the session summaries in their order, cut at a
  word; a newer reply not summarized keeps the last summary and says "Summary updating"; a session
  with none shows its recorded wording.
- A My Work read makes no provider call.
- A check is kept for its version only; a newer version asks again; Undo removes it; a stale version
  is refused; a forget of the session's records and a purge remove its entries.
- TeamBrain is not part of it.

## Not built

- Model-written per-request copy, stages and per-stream copy (claude-mem writes them only with
  OpenRouter as its provider; the owner's writes none).
- TeamBrain and claude-mem's cloud links.
- Server-sent events: the page polls, as it does for its other views (row 49).
