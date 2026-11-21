# The page: claude-mem's feed on oboete's engine

Unit C7 of the claude-mem parity plan (docs/cards.md, "Why"): the page the owner keeps open, a
feed of the cards (docs/cards.md), the session summaries (docs/summaries.md) and the prompts, newest
first, as claude-mem's viewer shows its observations, summaries and prompts. The behaviour of
record is claude-mem 13.28.0's viewer (`src/ui/viewer/`, `src/services/worker/PaginationHelper.ts`
at 039c6160), described for an independent implementation in the takeover notes'
`claude-mem-spec/viewer.md`. Nothing of its code is taken: the page stays `assets/viewer/` (plain
JavaScript and CSS, no bundler, no new dependency) and `src/view.rs`. claude-mem's My Work pages are
the one exception (decision 114, "claude-mem's My Work pages" below).

## Why

The experience check of 2026-10-03 (E4) set the two pages side by side: claude-mem's is a live
feed of session cards (the request as the headline, then what was investigated, learned and
completed) and observation cards; oboete's Timeline is a flat list of single claims and records
with 64-hex keys. The owner keeps the page open while working, so the feed is what memory looks
like to them.

## The feed (slice 1)

P1. **Where.** `Timeline` is the feed, after My Work and My Rules, My Work being the first tab since
slice 2 of X10 (decision 55, docs/my-work.md). The list it replaced (claims, imported history and
records, with the search box and its filters) stays as it is under a tab of its own, `Records`,
after it.
Context, Stats and Settings follow as now (built since: `Sessions` sits between Timeline and Records).

P2. **What.** Four kinds of item, each read through its one reader, so each is shown only as the
rest of oboete may show it:

- a card: `cards::read` (K4's hide rule, K6's gate);
- a session summary: `turns::read_row` (T7's hide rule, T8's gate), never a skip;
- a prompt: a `prompt` record of a live source (`raw::LIVE`), read through the raw reader that
  applies tombstones (D8), gated alone as a window gates a record. A removed record is not shown.
- a note an agent saved (`save_memory`, docs/tools.md slice 4; parity row N3): an imported
  document of source `oboete-note`, its newest row, gated on read as search and `get` gate it,
  never one a forget names. The page shows it as a card, labelled `Note`, with "Saved by an
  agent" as its subtitle and its text under `narrative`, as claude-mem's feed shows a manual
  memory among its observations (type `discovery`, subtitle `Manual memory`).

P3. **Order and pages.** Newest first by each item's own time (a card's window time, a summary's
turn end, a prompt's record time, a note's save); ties by kind (card, summary, prompt, note) and
then by ID, newest first. `GET /api/feed?repo=<key>|all=1&limit=<n>&before=<cursor>`, `limit` 50 by default and at
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
full refresh, not by polling. Nor does polling show a new item the first page does not hold: more
than a page of new items at once, or one whose own time is older than the page's last (a card of a
window long past); Refresh shows it in its place. claude-mem streams new rows over SSE; the viewer answers each request
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
`Completed`, `Next steps` and `Notes`, each only when it has text, newlines kept; the footer
`S<op seq>` (`S<device>.<op seq>`) and the local time. Notes since #403, as claude-mem 13.34.2's
card shows them.

P8. **The prompts.** claude-mem's prompt card: a `Prompt` badge, the agent, the repository; the
text whole, newlines kept; the footer: the record's key as `get` takes it and the local time.
Since 2026-10-25 (parity row X9), as claude-mem's `PromptCard` (#4757): a prompt of more than 8
lines or 600 characters (UTF-16 units, JavaScript's `length`) shows 8 lines, with `Show more`
under them (`もっと見る` in Japanese), which shows it whole and becomes `Show less`
(`折りたたむ`); the button says which in `aria-expanded`. The feed sends the whole text, which it
cut at 2,000 characters before.

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
| Delete buttons on observations and summaries | forget on a session, prompt, note or claim; hide on a card or summary | Forgetting is milestone 5's (spec 5.8), with its fence and its log (built since: docs/page-forget.md, docs/card-hide.md) |
| Live rows over SSE | polling every 3 seconds | the viewer answers each request and closes; a new record is seen within seconds either way |
| Pages by offset per kind | one cursor over the three kinds | no skipped row after a removal |
| Header links to its site, X, Discord, GitHub stars and a paid trial | none | they are claude-mem's own |
| Settings in a dialog with a preview column | the Settings and Context tabs | oboete's settings page (#94, docs/spec-webui.md) covers more than claude-mem's dialog; Context is the preview |
| Console drawer of the worker's log | none yet | the Stats tab shows the worker's state; a log view follows if the owner wants it |
| English only | Japanese and English | owner decision (#94) |
| Icons beside the summary's sections | none | its icon files are not taken; the labels say the same |
| The first visit's help shown again by `?` | the same | — |
| A session's title: its custom title, else the project | its first prompt, else the repository | oboete records no custom title; the first prompt says what the session was for |
| Sessions paged by offset, 200 at a time | by the last one's place, 50 at a time | as the feed (P3): none skipped or repeated when a session begins between pages |
| A session's menu deletes it | "Forget this session" on its page | docs/page-forget.md (parity row 3) |

## Sessions (slice 2)

Parity row 16: claude-mem's `Sessions` tab (`SessionList.tsx`, `SessionCard.tsx`,
`SessionDetailPage.tsx`, `CategoryFilter.tsx` and `/api/sessions` at v15.0.2): one card per
session, the newest first, and a session's page of its items, filtered by kind.

S1. **Where.** A `Sessions` tab (`会話`) after `Timeline`. The header's repository select applies
to it as to the feed.

S2. **The list.** `GET /api/sessions?repo=<key>|all=1&limit=<n>&before=<cursor>` answers
`{sessions, next}`: one entry per agent and session with a prompt hooks recorded (a live source,
as the feed's prompts are) that no removal took whole, as claude-mem's session begins at its first
prompt. Each has its key (16 hex digits of a hash of its agent and session, which names it in the
requests below, the same whichever of its prompts a forget takes), its agent, its session id, its
repository (key and display name, as the feed names it), its title, its start and its number of
items. Its title, start and repository are its first such prompt's: the title on one line, at
most 100 characters with `…` past them, gated as the feed gates it, and the repository's display
name when the prompt has no text (claude-mem's title is a custom title, else the project). Newest
first by start, ties by the first prompt's device and seq; `limit` 50 by default and at most 100
(claude-mem's 200 and 1,000, by offset). `next` is the last entry's place, so no session is listed
twice or skipped when one starts between pages. The number of items counts the session's current
cards (whose window is that session's alone, docs/cards.md K2, and none a recuration replaced),
its summaries that are no skip, and its prompts that no removal took whole; a card or summary a
removal hides (K4, T7) is counted until the worker passes it.

S3. **A session's card.** The title; the session id, its first 8 characters with the whole id as
the tooltip; `N items` (`N 件`); the repository's display name, the agent and the local start
time. The card is a button: a click or Enter opens the session's page.

S4. **A session's page.** `← Back to sessions` (`← 会話の一覧へ`) returns to the list as it was,
the opened card focused again; then the title, the agent and the session id; the kind filter (S5);
then the session's items as the feed shows them (P2 to P9), newest first, read by
`GET /api/feed?session=<key>` with the feed's paging, live updates and states, and no repository
filter. A key of no listed session has no items. A
session's items are its cards (K2), its summaries and its prompts. A note an agent saved belongs
to no session, nor does a card of a window of several sessions: neither is among them.

S5. **The kind filter.** One chip per kind among the items read so far, in claude-mem's order:
Prompt, Summary, then the card types (discovery, decision, bugfix, feature, refactor, change,
security_alert, security_note; other for a type outside them), each with its count. A chip turns
its kind on or off; with none on, every item shows. Beside them, `N of M` items shown (`M 件中 N
件`), with `+` while more pages remain.

S6. **Live.** A version change reads the list's first page again, which takes the place of the
sessions it covers: those begun since on top, one changed since (a removal took its first prompt)
in its new place, and one no longer listed (a removal took all its prompts) gone; the pages read
after it stay as read. A first page that does not reach the sessions shown (more than a page begun
since) starts the list again, More reading on from it. On a session's page, the session's new items
go on top as the feed's do (P5).

S7. **Safety.** As P12: the token, Host and method checks, each reader's gate and `gated` over the
answer, text nodes only, nothing written. The session id and the agent are gated as a card's
labels are (K6). A session's key in a request is compared, never used as a path.

S8. **Built since.** claude-mem's session menu deletes a session and everything captured in it;
oboete's session page has "Forget this session" (docs/page-forget.md, row 3).

## Tests of slice 2

1. `/api/sessions` lists each agent and session with a live prompt, newest first by its first
   prompt, with its key, title, items and repository; a session of imported records only is not
   listed, and a skip or a card of a window of several sessions is no item; `next` reads the
   rest, none twice and none skipped when a session starts between pages.
2. The repository filter keeps that repository's sessions; `all=1` gives every repository's.
3. A rule added after a prompt was written masks the title; a first prompt a removal took whole
   is no title, no start and no item, the next one giving them and the session keeping its key; a
   session whose prompts a removal took whole is not listed.
4. `/api/feed?session=<key>` gives that session's cards, summaries and prompts and no other
   session's, paged as the feed is; a key of no listed session has no items, and one that is no
   key is refused.
5. The page (the viewer's Node harness): the tab lists the sessions; opening one shows its items,
   the chips with their counts, and a chip filters them; Back returns to the list with the opened
   card focused; in both languages.
6. Without the token, with a foreign Host or another method, `/api/sessions` is refused as every
   route is.

## Tests of slice 1 (through `view::Viewer::route`)

1. `/api/feed` gives the three kinds newest first, ties by kind then ID, at most `limit`, and its
   `next` reads the rest; no item twice and none skipped when items are added between pages.
2. The repository filter gives that repository's items of every kind; `all=1` gives every
   repository's.
3. A card hidden by a removal, a summary hidden by a removal or skipped, and a removed prompt are
   not given; a prompt of an imported source is not given.
4. A rule added after a card, summary or prompt was written masks it in the answer.
5. A prompt over 2,000 characters comes whole in the feed, and `get` with its key gives it whole
   too; the page clamps a long one behind `Show more` (P8).
6. Without the token, with a foreign Host, or with another method, `/api/feed` is refused as every
   route is.
7. The page (checked in a browser, `assets/viewer/`): the feed with its kinds, the toggles,
   the filter, more pages, a new item appearing within seconds, the first-visit dialog and `?`, in
   both languages, in light and dark.
8. A note an agent saved is given by its time and repository, paged once, its newest text gated
   on read; one a forget names is not given and takes no place before the consumer catches up
   (`feed_shows_saved_notes_scoped_paged_and_forgotten_ones_hidden`). The page draws it as a card
   labelled `Note` (the viewer's Node harness).

## claude-mem's My Work pages (decision 114)

Decision 114 (2026-11-17) serves claude-mem 15.19.0's work-view (My Work, focus areas, Suggested,
Past focus and My Rules) on oboete's engine. The findings behind it, and what a spike checks, are in
the study of serving claude-mem's web UI from oboete (2026-11-17), kept outside the repository. The rest of this page is oboete's own, as above.

- Only the work-view is taken. claude-mem's classic viewer is not: its paths (`/api/settings`,
  `/api/sessions`, `/api/search`, `/api/timeline`) are oboete's with other shapes, it routes on the
  hash that carries the page's token, it deletes with `DELETE`, and its settings are its own; oboete's
  page already has those screens (Claude; overrulable).
- A throwaway spike in the isolated user comes first. If it fails, oboete's own page is extended by
  hand to claude-mem's pages instead (Claude; overrulable). The spike runs beside the remaining speed
  work of the page (Claude; overrulable). It passed on 2026-11-18, and each file its bundle holds had
  a confirmed licence, so none is replaced ([spike/work-view.md](spike/work-view.md)).
- The new pages open at an address of their own, `http://work-view.localhost:{port}/work-view/`
  (decision 116), with a token made from the viewer's that reaches their two reads and the writes
  of My Work they make, each on their origin alone, and nothing else of the viewer's (decision
  117); `oboete view` prints it under the main page's address. Its known limits: Safari before macOS 26 is
  reported not to resolve names under `.localhost` (measured once oboete runs on a Mac), and the
  viewer listens on 127.0.0.1 alone, so another program listening on `::1` at its port would get
  the pages' address, and in it the work-view's token, which reads My Work and makes the writes
  the pages take (decision 117), never the main one.
  Every action on them works there, as in claude-mem. Every package their bundle holds is checked
  before it ships and again at each update: where it comes from, what it runs when installed (the
  bundle is built with install scripts off), and the advisories against it. The page may reach no
  address but oboete's own.
- The pages read `GET /api/work-view`, claude-mem's snapshot of the work (`src/work_view.rs`). It is
  made from the answers of `/api/my-work` (the requests of the last 7 days), `/api/my-goals` and
  `/api/my-rules` as their readers give them, each text gated at its source, and from the Build
  story's kept texts, and reads nothing else. It goes through the gate in one walk as it leaves (#1089): each text as
  it is sent, and the Build story whole, read before any mask: each kept text as it was kept, as
  the gate reads a stored text; and the views the pages receive and show (the kept texts joined,
  each block of both as the pages split it, whole and with a heading's `#` marks off, and the story
  as sent), each read so too and with its edges as they are (the pages keep a U+0085 the gate's own
  reading trims). Then each view but the kept texts is read again as the masks leave it, until the
  rules find nothing new. A rule that matches only a part of one stored
  text the story cuts out, or only stored texts it joins, and that the stored text's own gate leaves
  as oboete's own page does, is a known limit (decision 115). No rule on a key takes what the pages read by it:
  the counts, days and times are read by the keys the code names. A request or conversation whose
  id My Work masked, or whose id the gate would change in a shape the snapshot writes it in, goes
  by a reference, `m:` and the keyed print of its stored id, the same in every place the pages
  match it; no id My Work gives begins `m:`. A focus area's id (whole, within a goals item's
  `a:<id>`, and as a request's area) and an open ask's go so too, by the print of the id in a
  namespace of its kind; Other, the code's own placement, goes as its word. A work item's status goes as one of the words the
  pages read, or as none. My Work and the goals gate their texts at their source under the home's
  rules, and the references and the walk read with the rules the answer leaves through; both sets
  are read before the stores and again once they are read, and the stores are read again under the
  new sets when either changed meanwhile, so that a rule saved during the reads is in force in each
  shape a text is gated in. The
  pages poll `GET /api/work-view/version`, the page's version, and read the snapshot again when it
  moves. Both answer on the pages' own origin alone, with its token (decision 116), asked again
  once the answer is read, as after a write (W4). What
  oboete keeps in other shapes is mapped: a work list is a workstream (one per repository, known by its
  place in its request), the
  weekly chapters of the Build story follow its recap, a finished month of goals is a period of Past
  focus, a conversation's own request is known by the conversation's key and titles the
  conversation, a request is complete when its newest status completed it (or, shipped by a review
  alone, on the review's day), a request's two newest reviews are its readings, from which the pages
  measure the pace oboete's page shows, and the goals' times are this machine's. What the snapshot does not carry yet is left empty: the deliverables
  (oboete's own page shows them), the evidence links (X37) and continuing a conversation (X36).
  My Rules carries oboete's (#1083): the suggestions waiting, and the owner's rules, an approved
  one by its suggestion, each under Everywhere or the repository it is for, the repositories a
  rule is for being the scopes. Each goes by the whole keyed hash of its key: a suggestion's rule
  by `l:` and its id's, one the owner wrote by `u:` and its uid's, a repository's scope by `r:` and
  its key's, the same whatever else a read holds, so that no key, uid or suggestion id leaves. The walk leaves these ids, who set a rule, its tier, its condition (always
  empty: X38) and its times whole, as the code's own words, and gates its statement, its scope's
  name and its quotes; two scopes whose names the gate leaves alike read apart on the page by as
  much of their ids as tells them apart. My Rules is read under the rules My Work is read under,
  in the same readers' hold. A rejected suggestion is a row the writes find (#1083's R2b), which the
  snapshot sends with the pull request that brings its Undo to the pages; an approved one whose rule
  was retracted or forgotten since has none, and oboete's own page undoes it. A rule's switch turns
  it on or off from the pages (#1083's R2a, `POST /api/work-view/rules`, `{action, ruleId,
  ruleVersion}`, `turn-on` or `turn-off`): each rule carries a version (`ruleVersion`), the keyed
  hash of what its row was read from (its claim's derivation, the correction the `active` view takes
  for each of its status, body and mute, an approved rule's decision, a suggestion's kept learning op
  and its newest decision op of any action, so that an undo leaves it at a version no read gave it
  before, the raw.db file), and a switch
  whose rule is not at the version it posts is refused (409 `stale`) and records nothing, so that an
  Undo after another change of the rule undoes none. The switch takes the locks W4's Mark finished
  takes (config.lock with the token asked again, raw.db and the readers' hold), reads My Rules on the
  handles it appends on, and appends its correction against a fence of what it read of the rule (an
  op naming its claim, an exclusion, a tombstone, a forget, another raw.db file; curation's other ops
  leave it be). A claim op names it by the uid it derives or by its `supersedes`, a correction by its
  `uid`, a rule decision by its `claim` or by the rule's suggestion (`about`); a claim that only
  quotes the uid does not (#1128). Before its locks the switch waits for the claims consumer past
  this device's newest correction and for the my_work consumer past its newest rule decision; and
  where a correction a worker of an older oboete passed waits, a viewer that runs its own passes
  runs one, whose claims step keeps it. Once the posted version and the rule's state are checked,
  it is refused as pending (409) while a claim or correction op naming the rule is past the claims
  consumer's checkpoint, a rule decision of it or of its suggestion is past the my_work consumer's,
  a correction of it that an older worker passed waits, or a tombstone is past the anchors
  consumer's. The viewer's save lock
  is held from the switch's locks to its append: through its waits for those locks, each within
  its own time, and through none of its waits for a consumer. As many switches as may wait for
  that lock are let in at once (503 busy past them). Applied and kept, its answer names the version
  it left the rule at (`savedRuleVersion`), which the receipt's Undo posts back, null when a record came after it that
  also decides the rule, and the pages then offer no Undo; not applied yet, it is saved, and its
  receipt says so, with no Undo.
- A suggestion's decisions take the same route, the same locks in the same order and the same
  refusals (#1083's R2b): `approve` and `dismiss` a suggestion that waits, `restore-suggestion` a
  rejected or an approved one (claude-mem's one Undo of a decision), each at the version it posts.
  The pages do not post them yet: their controls come with the pull request that shows them. An
  approval takes the suggestion's own stored statement as the owner's preference of every
  repository, as the main page's plain Approve does, and writes its directive, preference and
  decision in one transaction against the fence; an undo of an approved rule retracts its preference
  and undoes the decision in one append, the decision alone where raw.db proves the preference
  forgotten, and is refused as pending while the preference is neither in knowledge.db nor
  forgotten. A decision's write is refused, nothing recorded, when since the check a decision of the
  suggestion, a rules job's op of it, or a correction of its rule's claim was appended, and when a
  session it was learned from recorded an event in a repository the exclusion list held (that
  session is then excluded, and what was learned from it with it): this device's records since the
  check are read inside the write, at most 64, more refusing too. Its answer's version is the one
  a read of My Rules after the write gives the row by its reference, only while the row is the one
  the write left (an approval's preference derived by its own op with no correction deciding it);
  else none. The other controls that write a rule are not shown (below); a quote's source record is named as a prompt a review quoted, dated by
  that review, the quote itself shown whole; a rule the owner wrote is theirs, and a suggestion is
  learned by oboete. claude-mem's rules file and where its edits go are not shown: oboete writes no
  such file, and gives the rules that are on to Claude Code, Codex and Grok with the context it gives each
  session (where `[inject]` gives one), a repository's to its own sessions. A rule's age, which orders the rules by strength, counts in
  whole days in the mapped content the pages compare to decide whether a mapping is sent to the
  screens again (0008). Whether a model the screens are shown is newer work, which a save in flight
  then gives way to, goes by the content of the snapshot it was mapped from (0009, #1124), without
  what each read stamps on it (its times, its revision, each goals list's `updatedAt`) or what a
  save adds to its answer: the minute's mapping of the snapshot on screen, and a poll of the same
  content, are shown with their clock-made labels moved and are no newer work, so a save in flight
  keeps its answer when a rule's age passes a whole day or the date passes midnight, and a read in
  flight keeps its answer when the minute maps the work on screen again. A read still gives way to
  any snapshot accepted while it waited, as claude-mem orders reads (handing back the mapping on screen when that snapshot's content is the one on screen, #1127), and a save's answer is shown over
  a poll of the content on screen that came after an Undo in another tab: neither knows which saw
  the newer state, which a version taken with the state is to decide (#1126).
  A request with a judge's open grouping suggestion carries it, with its binding, and its Join and
  Not this save (W3b-3, docs/my-work-model.md slice 11b-2).
  The subagents of a conversation a request of the answer carries go as agents of that
  conversation, after every conversation's, while they have not finished and for 15 minutes after
  they finished (decision 118; older ones stay on oboete's own page), each with its conversation's
  label, title, project and content session id, and their Mark finished saves (W4,
  [subagents.md](subagents.md), "The work-view's Mark finished"). Its known limits: a stop that is
  kept, or that already waits to be written back, changes nothing on the card until a hook writes
  it back (the pages keep the model and drop the receipt), and its button records nothing more;
  two subagents of one conversation read alike in 15.19.0's pages (claude-mem 16 adds the launch
  description); more than 300 within the window leave the oldest out; a subagent's folder is
  looked at only when it is marked, so a card can offer Mark finished that answers that its
  conversation records nothing now; and a card changes when the pages read the snapshot again,
  after a write of theirs or when the page's version moves (above), which a subagent's steps, a
  stop a hook records and the passing of time (a subagent turning quiet, a finished one leaving
  the window) do not move, where claude-mem's pages read again within a second of a recorded
  change.
- The new My Work stays out of the page's menu until its Japanese is whole; its address opens it
  (Claude; overrulable). It shows the requests claude-mem's shows, with no switch of period; if
  claude-mem shows every request, the last 7 days while #1037's reads are slow
  (Claude; overrulable).
- A request shows claude-mem's states. What becomes of oboete's own (stalled, not started, a recorded
  update) is decided when oboete's old My Work pages go (Claude; overrulable).
- The focus areas' artwork ships as claude-mem's: its author added it to the repository, under
  Apache-2.0, and oboete's `NOTICE` names it with the rest of the bundle (Claude; overrulable). Each file the bundle holds is checked for
  its licence in the spike, and one whose licence is not confirmed is replaced (Claude; overrulable).
- oboete follows claude-mem's releases. When one bump takes more than a day, bumps come less often;
  the pages are not forked (Claude; overrulable).
- The timeline, records and settings take the new look as soon as the new My Work has its Japanese
  (Claude; overrulable).
- What the pages have and oboete lacks is hidden until it is built: rows X36 to X38 of
  [claude-mem-parity.md](claude-mem-parity.md). Continuing a conversation will work from the new
  pages directly too. Focus sharing and Team stay unbuilt (row 22). A control whose write oboete
  does not take yet is hidden until the pull request that takes it: My Rules' writes but a rule's
  switch (approving or rejecting a suggestion, by its buttons or a phone's swipe, editing a rule and
  adding one: #1083), and changing an area or a suggested one (#1085, #1102). Refresh now, a
  request's check and its Undo, the Join and Not this of a grouping suggestion (decision 117), and a
  rule's switch with its receipt's Undo (decision 114, #1083's R2a) work there. A write the viewer saved is told as saved there, its answer a snapshot or
  claude-mem's receipt alone, and a poll of the content already on screen that lands while it waits
  does not stand for it (#1108). claude-mem's installer is none
  of the pages and is left out of the bundle: oboete's setup is `oboete setup` and its settings page.
- The page's words are [GLOSSARY.md](../GLOSSARY.md)'s. In English a claim is a memory, and their list
  is Memories, since Learned is a summary's section, as in claude-mem (Claude; overrulable).
