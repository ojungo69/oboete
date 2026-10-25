# Forget from the page (parity row 3, milestone 5 slice 5)

The page forgets what `oboete forget` forgets, with the same preview and the same confirmation:
a session, a prompt, a memory (a claim) or a note, irreversibly, from raw.db, everything made from
it, and the backups (milestone 5 D5). spec-webui.md asks the page to keep the forget flow and its
semantics; this is the page's side of it.

## What claude-mem does

claude-mem 15.1's viewer deletes an observation, a summary or a prompt from the feed
(`DeleteButton`, "Delete this {item}? This cannot be undone.") and a session from its card
(`SessionCard`, "Delete this session and everything captured in it ({n} memories)? This cannot be
undone."), each with `window.confirm`, through `DELETE /api/observation/:id`, `/api/summary/:id`,
`/api/prompt/:id` and `/api/sessions/:platformSource/:contentSessionId`
(`src/services/worker/http/routes/DataRoutes.ts` at v15.1.0). An observation's delete removes its
row alone; its records stay.

## What oboete does

- **Two POST routes**, through the write guards every page write passes (`save_gate`: the Host,
  the token, the page's own Origin, JSON of a bounded size; the token checked again under the
  body): `POST /api/forget/preview` and `POST /api/forget/start`. No DELETE: the page uses none
  (milestone 4 D11's test).
- **The item, by the page's own id**: a session by its page key (its first prompt's record names
  it, as `oboete forget --session` takes a record), a prompt by its record (`<device>:<seq>`), a
  claim by its uid, a note by its uid. An id not of its kind's form is refused before anything is
  read (`forget_item`), and a session is looked for only in a store that exists: a home with none
  gets none made.
- **The preview** answers counts only: records, copies (records of the same sessions from another
  source, which may hold a copy and go with them), claims, cards, summaries, the claims their
  windows wrote (`written`, which lose their text until a recuration) and, for a claim, the
  claims it superseded, current again once it is gone (`again`); and `preview_key`, the
  preview's own token (`forget::Preview::token`: the target, the records, the copies and the
  denials). The page shows the item itself. What only the item's session can take is refused by
  name: a prompt whose copies take it past 500 records (`forget_too_large`) and a record of
  another device's (`forget_other_device`); the page says to forget its conversation instead.
- **The start** takes the item, the key and `confirmed: true`. It reads the preview again and
  goes on only while its token is the key: a preview that changed is stale (409), as the CLI's
  start refuses one, and the same start twice registers once. `forget::start` checks the token
  once more under raw's lock. The records are hidden at once; the purge, which the CLI runs
  before it returns, starts in the background (`forget --continue`, `forget::continue_now`), so
  the claims and windows made from them go a few minutes later. The answer says when a purge
  already runs (`cleaning: false`): that one read its jobs when it began, so this one's starts at
  the worker's next look after it; and when a request log could not be written (`logged:
  false`), which the CLI says too.
- **The page** has Forget on a feed entry of a prompt or a note, on the session page, and on a
  claim's details: the preview's counts in sentences (the copies and the written claims when
  there are any, the claims that come back), what stays outside (the agent's own transcript
  files; a claim's records, which search still finds), what is not taken back (what an agent or
  a curator was already given, a session that starts during the cleaning), "This cannot be
  undone.", Forget and Cancel, in Japanese or English, as the CLI says them before it asks. A
  done forget reads the view again, a session page's heading too; nothing is hidden on the page
  by itself. Its result stays in the status line for 15 seconds through the live view's redraws,
  which the store's change brings and which clear the line (seen on the real page: it went after
  1.6 seconds).

## Differences from claude-mem

- **A card or a session summary is not forgotten alone.** It goes with its session or the prompt
  it came from. A card's window can hold other sessions' records, so forgetting a card's records
  would take theirs; and a card's text alone is no target oboete's forget has (a new one would
  rewrite its window op and every backup). claude-mem's observation delete removes the row and
  keeps the records.
- **What goes is wider and said first**: oboete forgets the records and everything made from
  them, and the backups, and the preview counts them before the person confirms.
- **No `window.confirm`**: the confirmation is part of the page, as My Work's review is
  (decision 55).

## Tests

- `settings::forget::tests::a_prompt_is_forgotten_with_its_previews_key_and_once`
- `settings::forget::tests::claims_notes_and_sessions_are_forgotten_by_their_page_ids`
- `settings::forget::tests::what_is_no_item_is_refused`
- `settings::forget::tests::a_claims_preview_counts_the_claims_that_come_back`
- `settings::forget::tests::a_session_asked_of_a_home_with_no_store_makes_none`
- `settings::forget::tests::a_start_says_when_its_purge_waits_and_when_its_log_was_not_written`
- `settings::forget::tests::what_only_its_session_can_take_is_refused_by_name`
- `forget::tests::a_selection_whose_copies_pass_the_cap_is_refused_at_its_preview` (by name)
- `view::tests::the_page_forgets_a_session_and_its_reads_show_it_gone`
- `view::tests::w3_privacy_and_claim_writes_pass_every_guard_before_store_work` (both routes)
- the viewer harness's forget check
