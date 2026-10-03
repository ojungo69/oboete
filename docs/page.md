# The page: claude-mem's feed on oboete's engine

Unit C7 of the claude-mem parity plan (docs/cards.md, "Why"): the page the owner keeps open, a
feed of the cards (docs/cards.md), the session summaries (docs/summaries.md) and the prompts, newest
first, as claude-mem's viewer shows its observations, summaries and prompts. The behaviour of
record is claude-mem 13.28.0's viewer (`src/ui/viewer/`, `src/services/worker/PaginationHelper.ts`
at 039c6160), described for an independent implementation in the takeover notes'
`claude-mem-spec/viewer.md`. Nothing of its code is taken: the page stays `assets/viewer/` (plain
JavaScript and CSS, no bundler, no new dependency) and `src/view.rs`.

## Why

The experience check of 2026-10-03 (E4) set the two pages side by side: claude-mem's is a live
feed of session cards (the request as the headline, then what was investigated, learned and
completed) and observation cards; oboete's Timeline is a flat list of single claims and records
with 64-hex keys. The owner keeps the page open while working, so the feed is what memory looks
like to them.

## The feed (slice 1)

P1. **Where.** The first tab, `Timeline`, is the feed. The list it replaced (claims, imported
history and records, with the search box and its filters) stays as it is under a tab of its own,
`Records`, after it. Context, Stats and Settings follow as now.

P2. **What.** Three kinds of item, each read through its one reader, so each is shown only as the
rest of oboete may show it:

- a card: `cards::read` (K4's hide rule, K6's gate);
- a session summary: `turns::read_row` (T7's hide rule, T8's gate), never a skip;
- a prompt: a `prompt` record of a live source (`raw::LIVE`), read through the raw reader that
  applies tombstones (D8), gated alone as a window gates a record. A removed record is not shown.

P3. **Order and pages.** Newest first by each item's own time (a card's window time, a summary's
turn end, a prompt's record time); ties by kind (card, summary, prompt) and then by ID, newest
first. `GET /api/feed?repo=<key>|all=1&limit=<n>&before=<cursor>`, `limit` 50 by default and at
most 100, answers `{items, next}`: the server reads at most `limit` of each kind after that kind's
own position in the cursor, merges them, keeps the first `limit`, and `next` carries each kind's
position after the last one of its kind kept (unchanged for a kind none of which was kept), or is
null when no kind has more. The cursor is opaque to the page. So a page never repeats an item and
never skips one, whatever was added since the first page (claude-mem pages each kind by offset,
which can skip a row after a delete).

P4. **The repository filter.** The header's repository select, `All repositories` first, then the
repositories as `/api/repos` lists them now. A repository's items are those whose `repo` is its
key. The filter applies to every kind.

P5. **Live.** The page polls `/api/version` every 3 seconds as it does now. When it changes, the
first page is read again and new items are put on top; an item already shown is not shown twice
(by kind and ID). Items no longer returned (hidden by a removal since) leave the list at the next
full refresh, not by polling. claude-mem streams new rows over SSE; the viewer answers each request
and closes, so polling stays.

P6. **The cards.** claude-mem's observation card:

- header: the type as a badge (its own word, `bugfix`, `feature` … or `card` when it has none),
  the agent (`claude`, `codex`, `grok` …, as recorded), the repository's display name;
- the title (`Untitled` when empty); the subtitle while neither view below is open;
- two toggles, `facts` (shown when the card has facts, concepts or files) and `narrative` (shown
  when it has a narrative). One at a time: opening one closes the other, a second click closes it.
  Facts: one bullet per fact, in order, then one badge per concept, `read:` and `modified:` with
  the files shortened as claude-mem shortens them (from `src/`, `docs/`, `plugin/` or `Scripts/`
  in that order of preference, else the last three parts). Narrative: the whole text, newlines
  kept, scrolled past 300 px;
- the footer: the card's ID as `get` takes it (`<op seq>.<n>`, `<device>.<op seq>.<n>` for another
  device's) and its local time.

P7. **The session summaries.** claude-mem's summary card: a `Session summary` badge, the agent,
the repository; the request as the headline (none when empty); then `Investigated`, `Learned`,
`Completed` and `Next steps`, each only when it has text, newlines kept; the footer `S<op seq>`
(`S<device>.<op seq>`) and the local time. `notes` is not shown, as claude-mem does not show it.

P8. **The prompts.** claude-mem's prompt card: a `Prompt` badge, the agent, the repository; the
text, newlines kept, at most 2,000 characters with `…` past them (the whole text through `get`);
the footer: the record's key as `get` takes it and the local time.

P9. **States.** `No items to display` when the filter has none; `Loading…` while a page is read;
`No more items` after the last. A failed read says so and offers to try again (claude-mem only
logs it). New pages load when the end of the list comes into view (an `IntersectionObserver`
sentinel), and by a `More` button for keyboards and browsers without it.

P10. **The first visit (P6 of the switch list).** A dialog over the page the first time it opens,
dismissed by its close button, `Esc` or a click outside, and remembered in the browser
(`oboete-welcome-dismissed`); a `?` button in the header shows it again. Three short parts, as
claude-mem's: the feed (cards, summaries and prompts appear here as the worker curates the work,
a few minutes after it), the settings (what a new session is given and who curates, in the
Settings tab), and recall (the agent asks oboete's `search`, `timeline` and `get`; the Records tab
searches here). In the page's language.

P11. **Language.** Every label, state and the first-visit dialog in Japanese and English, by the
page's language switch (#94); the cards', summaries' and prompts' own text as written.

P12. **Safety.** The API answers only through the viewer's token, Host and method checks; every
text reaches the page gated (each reader's gate, and `gated` over the whole answer as every route
does); the page writes text with text nodes, never as HTML. Nothing is written to a store.

## Differences from claude-mem, and why

| claude-mem | oboete | Why |
|---|---|---|
| Delete buttons on observations and summaries | none yet | Forgetting is milestone 5's (spec 5.8), with its fence and its log; the page gets it in M5 slice 5 |
| Live rows over SSE | polling every 3 seconds | the viewer answers each request and closes; a new record is seen within seconds either way |
| Pages by offset per kind | one cursor over the three kinds | no skipped row after a removal |
| Header links to its site, X, Discord, GitHub stars and a paid trial | none | they are claude-mem's own |
| Settings in a dialog with a preview column | the Settings and Context tabs | oboete's settings page (#94, docs/spec-webui.md) covers more than claude-mem's dialog; Context is the preview |
| Console drawer of the worker's log | none yet | the Stats tab shows the worker's state; a log view follows if the owner wants it |
| English only | Japanese and English | owner decision (#94) |
| Icons beside the summary's sections | none | its icon files are not taken; the labels say the same |
| The first visit's help shown again by `?` | the same | — |

## Sessions (slice 2)

claude-mem's `Sessions` tab: one row per session (its first request or the repository, the
agent, the number of items, the start time), the newest first, and a session's page: its items as
the feed shows them, with a filter by kind (`Prompt`, `Summary`, then the card types). Designed and
built after slice 1, before the switch.

## Tests of slice 1 (through `view::Viewer::route`)

1. `/api/feed` gives the three kinds newest first, ties by kind then ID, at most `limit`, and its
   `next` reads the rest; no item twice and none skipped when items are added between pages.
2. The repository filter gives that repository's items of every kind; `all=1` gives every
   repository's.
3. A card hidden by a removal, a summary hidden by a removal or skipped, and a removed prompt are
   not given; a prompt of an imported source is not given.
4. A rule added after a card, summary or prompt was written masks it in the answer.
5. A prompt over 2,000 characters is cut with `…`; `get` with its key gives it whole.
6. Without the token, with a foreign Host, or with another method, `/api/feed` is refused as every
   route is.
7. The page (checked in a browser, `assets/viewer/`): the feed with its three kinds, the toggles,
   the filter, more pages, a new item appearing within seconds, the first-visit dialog and `?`, in
   both languages, in light and dark.
