# Remove a card or a summary from the page (parity row 3, decision 57)

A card or a session summary can be removed on the page alone, reversibly, as a correction is:
removed, it leaves the page, session start, search and what agents are given, and it comes back
when the removal is undone (decision 57). Erasing one stays forgetting its session or the prompt
it came from (docs/page-forget.md), since a card's window can hold other sessions' records.

## What claude-mem does

claude-mem 15.1's viewer deletes an observation or a summary from the feed (`DeleteButton`,
"Delete this {item}? This cannot be undone.", `DELETE /api/observation/:id` and
`/api/summary/:id`, `src/services/worker/http/routes/DataRoutes.ts` at v15.1.0). The row goes;
the records it was made from stay, and nothing brings it back.

## What oboete does

- **A `hide` op** names the card or the summary and carries no text: `{target, device, op_seq, n,
  hidden}`, the target `card` (a window op's card, `n` its place there) or `summary` (a turn op's
  summary), `hidden` false to bring it back (`hide::HideOp`). It is an op like a correction, so
  the backup keeps it, a restore and a rebuild bring it back, and a rewind of the op log takes it
  away.
- **The consumers** keep each hide op in knowledge.db's `hides` (who wrote it, what it names, its
  mark) and mark the card or the summary: `cards.hidden`, `turns.hidden`, the newest op's mark.
  A mark takes the row's full-text entry out (`cards_fts`, `turns_fts`) and drops its vector
  (`embed_phase::touched`); brought back, the full-text entry returns and the embedding phase
  embeds it again (its vector is cached by text). A card or a summary written after a hide op that
  names it (a rebuild of knowledge.db) takes the mark the op left. A rewind past a hide op shows
  what it removed again.
- **Every reader for a person or an agent** leaves a marked row out: the feed, a session's page
  and its counts, the timeline, `get` (the page's details and the MCP tool), session start, the
  note on a file, the words and the meaning search, My Work. A summary's row stays in `turns`,
  so what was asked is not asked again; only its readers pass it over. A hook can read a store the
  worker has not migrated yet, so a reader asks for the column first (`hide::shown`).
- **One POST route**, through the write guards every page write passes (`save_gate`):
  `POST /api/hide` with `{item: {kind, id}, hidden}`, the item by the id the page shows (a card's
  `<op_seq>.<n>` or `<device>.<op_seq>.<n>`, a summary's `S<op_seq>` or `S<device>.<op_seq>`). It
  is refused before anything is written when the kind or the id is not of its form (422
  `hide_item`), when there is no store (no store is made), and when no current card or summary has
  that id (404 `hide_not_found`). The same state again writes no op. The answer comes when the
  consumer has applied the op (`claims::applied`), or says it is pending (`hide_pending`).
- **`GET /api/hidden`** lists what is removed, the newest first, each with its kind, id, title,
  repository and time, through the same gate as the feed, for the page's Settings to bring back.
- **The page**: "Remove from the page" on a feed entry of a card or a summary, on the timeline and
  on a session's page, beside the prompts' and notes' Forget. It asks nothing first, as it is
  undone in one step: the page reads the view again, and the status line says it was removed, with
  Undo, for 15 seconds through the live view's redraws; a refusal is held as long, so a reread
  does not clear why an Undo failed. Settings lists what is removed, with
  "Bring back": the detailed page under "Diagnostics and maintenance", and the simple page while
  something is removed. In Japanese and English.
- **The format**: a binary before this one stops at an op type it does not know, so the first hide
  op raises the home to format 4 (`raw::HIDDEN_FORMAT`) in the transaction that writes it, and a
  rebuild that brings one back stamps it too (docs/store-format.md). A home with no hide op still
  opens in the binaries before.

## Differences from claude-mem

oboete's removal is undone from the page; claude-mem's delete cannot be. A card's records stay in
both, and erasing them is oboete's forget.

## Limits

- A card removed, and its window curated again later (a forget of one of its records queues it),
  is replaced by the new cards, which are shown: the new cards are not the one removed, and their
  text may differ.
- The memories (claims) a removed card carried stay; each has its own correction, mute and forget
  on its details.
- Agents and curators already given a card before its removal keep what they were given.

## Tests

- `settings::hide::tests::a_card_removed_from_the_page_leaves_its_readers_and_comes_back`
  (session start, the feed's pages, the timeline, `get`, the note on a file, the cards a turn's
  summary is shown, the words search, the full-text entry; listed; kept through a rebuild of
  knowledge.db, which takes the newest op's mark; the same state twice writes one op)
- `settings::hide::tests::a_summary_removed_from_the_page_keeps_its_row_and_comes_back` (the same
  readers, its row kept, through a rebuild)
- `settings::hide::tests::a_hide_op_read_before_what_it_names_still_marks_it` (another device's op
  read first, the newest of two marks)
- `settings::hide::tests::a_rewind_past_its_hide_op_shows_the_card_and_the_summary_again`
- `settings::hide::tests::a_removed_card_loses_its_vector_and_waits_for_none` (doctor's count)
- `embed_phase::tests::a_card_and_a_summary_removed_from_the_page_lose_their_vectors_until_brought_back`
  (with the stub embedder: nothing removed is sent; brought back, embedded again)
- `settings::hide::tests::a_hide_raises_the_home_to_the_format_that_reads_it` (the append and a
  rebuild from the backup's lines)
- `settings::hide::tests::what_is_no_card_or_summary_is_refused`
- `view::tests::the_page_removes_a_card_and_brings_it_back` (through `/api/hide` and
  `/api/hidden`; the session's count)
- `view::tests::w3_privacy_and_claim_writes_pass_every_guard_before_store_work` (the route added)
- the viewer harness: Remove on a card and a summary, Undo through a redraw and a reread that
  writes the status line, Undo pressed before the removal's reread answered (its result, or why it
  was refused, stays), a refusal named, the pending answer, Japanese and English, Settings reading
  the list when it opens, both views' list, and Bring back

Mutation checks: each of the server's 33 checks (every reader's condition, the consumers' marks,
full-text entries and vectors, the rebuild's and the rewind's marks, the embedding phase's
filters, the same-state answer, the no-store guard, the format stamps) fails a test when removed
(33/33), and each of the page's 21 (Remove and its request, the result and its Undo held through
redraws and rereads, a refusal named and held, the Settings list in both views, its read, Bring
back) fails the viewer harness (21/21).
