# The page's reads: no repository list where none is used, and no view read twice

Findings W3 and W6 of the 2026-11-10 audit, on why the page felt slow to open and to move between
its tabs. Both are in what the page asks the viewer for (`assets/viewer/app.js`); the viewer's
answers are as they were.

## What happened

- The repository list (`GET /api/repos`) counts every repository's claims, imported documents and
  records on each read, with no cache: about 104 KB and 0.3 to 2.3 s on a store of 4 GB.
- Every refresh read it first and waited for it, whatever the tab:
  - the first paint asked for the version, then the list, then My Work, one after another,
    though My Work shows nothing of the list;
  - under Stats, which follows every poll, the list was read again every 3 seconds;
  - under My Work and My Rules, each change of the store read the list before the view.
- W6: a poll that had seen a change read the list and then the view. A click on a tab, or a
  search, made while the list was being read, read the view itself; the poll then read the same
  view a second time.

## The change

- `usesRepos()` says whether the view on the page reads the list. My Work, My Rules, Settings and
  Stats with no search do not.
- A view that reads no list does not wait for one. At the first paint, and on the Refresh button,
  its own read begins beside the list's (`refresh`), not after it.
- A poll under such a view reads no list (`reposStale`). The next view that reads the list reads
  it again beside its own read (`show`), so a repository made meanwhile is in the picker there.
  - Settings does the same as it opens, waiting for none: its preview of a session's start
    chooses among the list's repositories. A change seen under Settings marks the list unread
    too (`poll`); before, the list stayed as it was until the next change after Settings was left.
  - A choice the list no longer holds (a repository forgotten meanwhile) goes back to All
    repositories, and the view, asked for beside the list with the old choice, is read again for
    what the picker says.
  - Under Stats with no search the picker is on the page and reads no list. Each poll there
    marks the list unread, so a search or another tab then reads it, and a repository first
    recorded meanwhile is in the picker. One made within the 3 seconds before any poll saw it
    comes with the next poll's read of the list, under the view that reads it. The Refresh
    button reads the list at once.
- A list that fails is shown as before, and a read still on its way draws nothing over the
  error. While no list was ever read, the poll asks the list again each time, as before, and the
  list alone: the view is not read again every 3 seconds while the list fails (`reposFailed`).
  A list read before that fails stays marked unread (`loadRepos`): a poll under a view that
  reads no list asks for none after a failure either, and the next view that uses the list asks
  again.
- W6: `refresh` notes how many times a view's read has begun (`shows`) as it begins. If a click
  or a search moved it while the list was read, the poll's refresh ends there: the view was read
  after the version the poll saw. The count that guards drawing (`generation`) also moves where
  nothing is read (My Work's way back from a request, `routeShow`), so it is not the one used.
  Where the poll's list dropped the picker's choice, the click's read was for that choice: the
  refresh does not end there, and reads the view for what the picker says. The poll ends as the
  click's read ended (`lastShow`): where that read failed, the poll keeps its version, and the
  next one reads the view again (#903).

Unchanged: under Settings a poll reads the list only while none was ever read (it keeps unsaved
drafts and the note of a config.toml mistake by it); the Refresh button, a forget and a restore
read the list under any view; a view that reads the list has it read live at each change; every
answer still passes the viewer's masking gate (`view::answered`).

## Not in this change

- The list itself is still counted on each read (a cache on the viewer would need a key that
  moves with every table it counts). The page now asks for it far less often.
- My Work's own cost (W1) and the timeline's (W4) are the viewer's and come next (#899, with
  what each was measured at). The settings page's names (W2) are below.
- The first paint still asks for the version before anything else.

## Tests

`src/testdata/viewer-readiness/test.mjs` (run by
`view::tests::w6_agent_registrations_ui_preserves_drafts_and_reads_without_posts`), the block
"W3 and W6". Each view's read is replaced by one request that draws nothing, and the requests are
counted:

- at the first paint My Work is asked for while the list is still being read, once;
- three polls under Stats read the statistics three times and the list never; Stats with a
  search reads the list;
- a live refresh under My Work or My Rules reads no list, and the next view that reads the list
  reads it again, once;
- a view that reads the list has it read live; the Refresh button reads it under Stats too;
- a list that fails at the first paint leaves its error shown, the poll asks the list alone, and
  once it loads the view is read;
- a click while a poll reads the list: the view is read once, My Work opened then too; a count
  moved with no read behind it does not end the poll; a click whose read fails leaves the poll's
  version where it was, and the next poll reads the view (#903, with two mutants of its own);
- Settings opened on a list left unread reads it, once; a change under Settings reads no list
  and leaves it to the next view; Settings opened before any list was read leaves it to the poll;
- a choice gone from the list read beside the view: the picker is at All repositories and the
  view is read again for it, and the same where a poll reads the list while a click reads the
  view; a choice still listed is read once;
- a list read before that fails on the Refresh button: no poll under Stats asks for it, and the
  next view that reads the list does.

25 hand-made mutants of the page's change, each run against the harness, all fail it.

## W2: the settings page's names, on the viewer's side

The settings page's privacy section lists every repository by name (`settings::privacy`). To
have the names it did two things on every read:

- it read every record of raw.db for the repositories they name: 5.6 s the first time and
  0.62 s again on a store of about 4 GB;
- it ran the repository list's own query (`view::REPOS`: each repository's claims, imported
  documents and records counted, with its newest time) and kept the names: 3.1 s and 0.27 s.

Each query was timed alone: its first figure is a read with none of its pages in memory, its
second the same read again. The whole request (`GET /api/privacy`), three times in a row on that
store before either change, took 3.6 s (some of the pages were in memory), then 0.87 s and
0.84 s.

The first change is to the second of them. `view::repo_names_in` reads the names alone, from
the same three tables under the same filters, each from an index on its repository: 0.14 s the
first time and again. The page's rows are the same.

Measured, and not changed:

- Each name is checked against the redaction rules three or four times for a row (what a hook
  would store it as, twice or more, and what the page shows). On a made-up store of 950
  repositories and 3,800 records the whole request takes 13 to 37 ms on a release build, the
  checks included, so a check kept and used again would save a few milliseconds. #899's steps 3
  and 4 are not built.

The second change is to the read of every record:

- That read is what finds the repositories knowledge.db does not name: those of records not yet
  indexed, of another device's records (the index reads this device's alone) and of a rule's
  record (the index gives it no row). So it stays, and reads an index of raw.db's own:
  `raw::event_repos` walks `records_repo`, the least name and then the least one above it, one
  seek each, where that index is built, and reads every record as before where it is not.
- `records_repo` was built until now by the first exclusion or forget's preview that read the
  records by repository. The idle worker builds it too, in the pause of the owner's work in
  which it builds the session index (`worker::maintain`): its first build reads every record
  once, which a hook's write would wait out. A reader builds none.
- Measured on a made-up raw.db of 750,000 records (1 GB) that name 17,020 repositories: every
  record read, 0.6 to 2.2 s; the index's build, 1.0 s and 29 MB; the names by seeks, 0.02 s.
  A hook's write with the index in place shows no difference in `oboete replay`: a spawned hook
  took 13.7 and 14.4 ms with it and 14.1 and 15.0 ms without, in two rounds each.

Tests: `view::tests::the_repositories_names_are_the_lists_names_read_from_indexes` (the names
are the list's, a repository in one table alone too, no name twice, a record of no repository no
name, each table read from an index);
`settings::privacy::tests::the_names_only_knowledge_holds_are_listed_and_masked_as_shown` (a name
only knowledge.db holds is listed, one a rule masks is shown masked under the selector of the
name as stored, and no repository is counted for the page);
`raw::tests::the_events_repositories_are_read_from_their_index_once_it_is_built` (the same names
by the index as from every record, by seeks and with no read of the records, a name added after
too); `settings::privacy::tests::repositories_the_index_never_names_are_listed_from_the_records`
(a rule's record's repository and another device's are listed, with the index and without, with
knowledge.db and without, and a read builds no index);
`worker::tests::the_pause_builds_what_it_keeps_on_a_store_with_any_part_of_it`.

## The cards' part of W4: a page of cards is read in an index's order

W4 is the timeline's whole cost, which stays with #899. This is its cards, and the same statement
is the first step of a My Work read.

Measured on 2026-11-10 with `main` at 5e775fda, on this PC's store (43,767 cards). A read of My
Work (`my_work::listed`, every range) took 550 to 575 ms inside the process, timed step by step:

| Step | Time |
|---|---|
| the newest 300 cards (`cards::page`) | 123 to 132 ms |
| the newest 100 summaries | 45 to 54 ms |
| the work-state writes and their links | 42 to 48 ms |
| the prompts' labels, then their texts | 33 to 36 ms |
| the requests' answers | 108 to 123 ms |
| the copies and the products | 75 to 87 ms |
| the subagents | 85 to 104 ms |

The cards came first. `cards::page` asks for them the newest first (`ts`, `device`, `op_seq`, `n`,
all descending), with one statement for every page: every repository's, one repository's, one
session's, from the start or after a card (`?1 IS NULL OR repo = ?1`, and so on). One statement has
one plan, and the table had no index in that order: `SCAN cards` and `USE TEMP B-TREE FOR ORDER
BY`. Every card the store holds was read and what the filters left was sorted, for a page of 300.
`cards::around` ran the same plan. A read grew with the store, not with the page.

Two things changed.

**A statement names only what it filters on** (`page_sql`, `around_sql`): the repository when one
is asked for, the session when one is, the card to start after when there is one. Each shape has
its own plan.

**Each shape has an index in its order**, made by `cards::schema` where it makes the table:

| A page of | Read through | Sorted |
|---|---|---|
| every repository's cards | `cards_newest (ts, device, op_seq, n)`, new | nothing |
| one repository's | `cards_repo (repo, ts)`, as before | the cards of one `ts` |
| one session's | `cards_session (agent, session, ts)`, new | the cards of one `ts` |

A page after a card does not start at the newest. Every repository's seeks to that card on all
four of its columns (`(ts, device, op_seq, n) < (...)` in `cards_newest`). One repository's and one
session's seek to the card's `ts`, which is as far as their indexes go, then pass over the cards
of that `ts` at or after the card and sort the rest of them. The cards on either side of a time
are read the same way, through `cards_newest` or `cards_repo`.

On a copy of the table in memory, a page of 300 with every column:

| A page of | Before | After |
|---|---|---|
| every repository's cards | 85 ms | 0.2 ms |
| the repository with the most cards (23,059) | 21 ms | 0.3 ms |
| a repository with 20 cards, none of them recent | 8 ms | 0.1 ms |
| a session with 10 cards, none of them recent | 7 to 8 ms | 0.1 ms |
| every repository's cards after the 30,000th | 85 ms | 0.2 ms |

The two new indexes took 16 and 24 ms to build there. With `cards_newest` alone and the one
statement kept, the repository with 20 cards and the session would have gone to 13 and 12 ms: the index's order
passes over every other repository's cards one at a time, which is why the statements were split.

An existing store gets the two indexes when `cards::schema` next runs on it: at the worker's next
summary or embedding pass, or at the first search, whichever comes first. A reader makes no
index, and one that finds none reads as before.

The other steps of a My Work read are not looked into here (#899).

Test: `cards::tests::the_newest_cards_are_read_in_the_order_of_an_index` (every shape of the
page's statement and of `around`'s is read through the index in its order, none sorts more than
the cards of one `ts`, every repository's page sorts nothing, and a page after a card seeks to
that card on its four columns, or to its `ts` for one repository's and one session's).

## W1: a request keeps one clone of each rule's regex

The same read, by its CPU: a profile of the viewer over six My Work reads (7,120 samples) put a
fifth of it, 20.6%, in the regex crate, most of that in building a lazy DFA's states
(`determinize::next`, `epsilon_closure`, `add_nfa_states`).

`redact::Compiled::get` hands out a clone of the cached `Regex` for every use: `spans_in` once for
each rule whose keyword is in the text, `allows` once for each allowlist regex of each candidate.
A clone shares the compiled program and has a cache pool of its own. So every use allocated a
cache, built the DFA's states from nothing, searched and freed it. #888 made a pattern compile
once in the process; the cache was still thrown away at every use.

While a `redact::Warm` lives on a thread, that thread keeps one clone of each regex it uses and
uses it for every text; the last `Warm` to go takes them with it. The viewer serves a request on
a thread of its own, and `serve` holds a `Warm` for it, so a request's regexes are built once for
all of its texts and are gone when it is answered. A thread with no `Warm` gets a clone for each
use, as before.

What was weighed against it, on a harness apart from oboete (regex 1.13.1, the bundled rules,
176,987 lines of this repository as the texts, 37,667 rule uses):

| How a use gets its regex | Time | Memory kept after the run |
|---|---|---|
| a clone for each use (before) | 0.46 s | +6 MB |
| one regex shared by every thread | 0.11 to 0.18 s | +10 MB on one thread, +39 MB after 16, +60 MB after 64 |
| one clone a thread, dropped with the thread | 0.11 to 0.14 s | +10 MB while the thread lives |

One regex for every thread keeps a cache for each of its pool's stacks, for as long as the process
lives; a resident process cannot take that. A clone kept for a request is let go with it.

That is not nothing left behind. Measured after the merge, on this PC's store, with each read of
My Work on a thread of its own, as the viewer serves each connection: after 40 reads the process
held 174 MB when every read had a `Warm` and 161 MB when none had, and neither grew over the next
20. A `Warm` adds 12 to 13 MB to where the process's memory settles.

Most of what settles there is not the `Warm`'s but the threads'. Two more runs of 40 reads, every
read with a `Warm` in both, counting anonymous memory alone: with each read on a thread of its
own the process held 152 MB, and with all 40 on one thread it held 37 MB (#922).

On this PC's store, a My Work read inside the process, with a `Warm` and without, three of each
in turn: 479 to 512 ms against 550 to 575 ms. The requests' answers went from 108-123 ms to
55-62 ms; the other steps did not move. That is 11% of the read, not the 20% of the profile: the
rest of the regex share is the module's own regexes on the prompts' texts.

Not given a `Warm`: a hook (three replays of the 1,000-event fixture with one and without showed
no difference: a hook's process compiles its rules at each start, which is its cost), the MCP
server's calls and the worker's passes (not measured).

Tests: `redact::tests::a_thread_keeps_one_clone_of_a_regex_while_a_warm_lives` (nothing kept
without a `Warm`; one clone for every use with one; one inside another ends nothing; the last to
go takes them; what does not compile is asked for once),
`redact::tests::a_threads_regexes_start_again_past_their_cap`,
`redact::tests::a_secret_is_masked_with_a_warm_as_without` (on the thread, on another thread, and
right after the thread's map started again).

## #1037: the prompts' fields, from an index of their own

What happened. My Work's list of prompts (`Raw::prompt_labels`) and the sessions of the Sessions
tab and Settings (`Raw::live_sessions`) read the prompts through `records_prompts`, which holds a
prompt's time, device and seq alone. Each read went on to every prompt's row for its source,
agent, session, repository and expiry, one leaf page a prompt, though most prompts are imported
and leave the read at their source. On this PC's store (6,876 prompts, 922 of them live), each
read took about 30 MB of a request: of My Work's 109 MB once the expired events' index is built,
of the Sessions tab's 105 MB, and of Settings' 31 MB. Attributed with a throwaway build that wrote
each statement's `read()` bytes on its thread, less those of the statements nested in it, never
committed.

The change. `records_prompt_fields` holds those fields beside the prompts index's order, with the
same condition. The worker builds it in a pause of the owner's work, as the expired events' index
(`worker::maintain`): its first build read 1.8 GB in 12 s on a copy of the store. Until it is
built the reads take `records_prompts` as before. Both reads, and the prompts since a time
(`Raw::prompt_seqs_since`), name the index they take (`Raw::by_prompts`), since with no statistics
SQLite breaks a tie toward the index made last (#1036).

On the copy, through the new index: the prompt list 30.5 MB and 120 ms to 0.8 MB and 2 ms, the
sessions 30.5 MB and 14 ms to 0.8 MB and 2 ms, the same rows.

Tests: `raw::tests::the_prompt_reads_take_each_prompts_fields_from_their_index` (the three reads'
plans read the index alone, with the indexes made in a live store's order and one made after
it; the same answers before and after, an expired prompt and one a forget took whole included;
a prompt that expires after the build), `worker::tests::the_pause_builds_what_it_keeps_on_a_store_with_any_part_of_it`.

## #1037: the removed lists and the sessions' counts, from partial indexes

What happened. Settings' list of what is removed from the page (`cards::removed`,
`turns::removed`) and the Sessions tab's counts of each session's items (`view::sessions`) read
every card's and every summary's row. The list of removed cards walked `cards_newest` to find the
few removed ones and read each card's row for its mark; the summaries had no index in a page's
order, so the list, and each page of summaries My Work reads (`turns::page`), read every row and
sorted them all. The count of cards took `cards_session` and read every row for `replaced_by` and
`hidden`. On this PC's store (57,749 cards, none removed; 2,666 summaries), measured statement by
statement in the requests of the page with the throwaway build of the prompts' fields above:

- the removed list: 68.1 MB of cards and 5.8 MB of summaries, for nothing to list
- the Sessions tab: 67.7 MB of cards and 5.8 MB of summaries, of its 105 MB
- My Work: 6.1 MB of summaries a page

The change. Four partial indexes, made by `cards::schema` and `turns::schema` after each table's
other indexes, in a new store as in one an older version made, so each store has them in one
order:

- `cards_hidden (ts, device, op_seq, n) WHERE hidden = 1 AND replaced_by IS NULL`
- `cards_session_shown (agent, session, ts, replaced_by, hidden) WHERE replaced_by IS NULL AND
  hidden = 0 AND agent IS NOT NULL AND session IS NOT NULL`
- `turns_hidden (ts, device, op_seq) WHERE hidden = 1 AND skipped = 0`
- `turns_shown (ts, device, op_seq, agent, session, repo) WHERE skipped = 0 AND hidden = 0`

The removed lists, the counts and a page of summaries name their index once the store has it
(`hide::indexed_by`), since with no statistics SQLite breaks a tie toward the index made last
(#1036), and read the tables as before until it has. The columns of the cards' condition are
columns of `cards_session_shown` too: SQLite takes a partial index's condition as known for only
some of its terms, and without both `replaced_by` and `hidden` among its columns the count of
cards still read every row. It takes both terms of the summaries' condition, so the count of
summaries reads `turns_shown` alone. `cards_session_shown` names the sessions in its condition,
so the feed's reads, which name none, cannot take it. Each first build reads its table once: on a copy of this
PC's store 55 and 60 ms for the cards' two, 7 and 8 ms for the summaries' two. They are built
where the schema is made, as `cards_unmarked` is: by the worker, the viewer, a search, retention,
forget and doctor. No hook makes it: of the functions codegraph's call graph has above
`cards::schema` and `turns::schema`, the 38 outside the tests include none of a hook's.

On the copy, the first read of each statement on a new connection, before and after:

| read | before | after |
|---|---|---|
| the count of cards | 67.5 MB, 28 ms | 2.9 MB, 4 ms |
| the count of summaries | 5.8 MB, 3.9 ms | 0.3 MB, 1.1 ms |
| the removed cards | 67.9 MB, 30 ms | 0.01 MB, 0.3 ms |
| the removed summaries | 5.8 MB, 3.0 ms | 0.01 MB, 0.2 ms |
| a page of summaries, every repository | 5.8 MB, 9.9 ms | 0.2 MB, 0.3 ms |
| a page, the repository with the most | 5.8 MB, 7.9 ms | 0.2 MB, 0.3 ms |
| a page, a repository with one | 5.8 MB, 3.6 ms | 0.3 MB, 0.5 ms |
| a page, the oldest session's | 5.8 MB, 4.5 ms | 0.3 MB, 0.5 ms |

The same rows in the same order before and after, every column compared: on the copy, and on
the copy with every 97th card and every 13th summary marked removed and every 89th card
replaced, where the removed lists held 573 cards and 184 summaries. Every other statement on the two tables was listed twice,
independently, the lists checked by a third reader, and each planned before and after on an
empty copy of the store's schema made in its order. No plan of the cards' changed but the two
above: their page and the cards around a time keep theirs, as
`cards::tests::the_newest_cards_are_read_in_the_order_of_an_index` holds. Four of the summaries'
now take `turns_shown`, and each reads less:

- the summaries around a time (`turns::around`), by their time instead of every row sorted
- the embedder's page of summaries (`embed_phase::read_page`)
- doctor's count of the summaries waiting for it
- the full-text consumer's read of one summary (`consumer::fts::turns`), which read every row
  for each summary it was given

The one read that can take more is that consumer's backfill, which runs once, when a store makes
`turns_fts` (#403): it reads each shown summary's row in the index's order. On a copy of this
PC's store that took about as long as before. On a made-up store 40 times as large, its rows in
no order of time, it took up to 1.75 times as long.

Tests: `cards::tests::the_removed_cards_are_read_from_an_index_of_their_own`,
`turns::tests::the_summaries_pages_are_read_from_indexes_in_their_order`,
`view::tests::the_sessions_counts_are_read_from_indexes_alone` (each with the indexes made as a
live store makes them and ones made after them that SQLite would take otherwise; from a store
without them, the statements name none). What the counts and the lists answer is
`view::tests::sessions_list_each_live_session_by_its_first_prompt`'s and
`settings::hide::tests::settings_lists_every_removed_card_and_summary_page_by_page`'s, which read
through the indexes.

## #1037: each leg of the timeline reads its own page

What happened. The timeline (`search::b::timeline`, which the Timeline tab, the MCP timeline tool
and `near` read) read, for each page, the union of every current claim and every session start,
sorted, with the imported documents already read to the page's end (N5). A repository's filter
was written `?1 IS NULL OR repo = ?1`, which makes one plan that reads them all. So the claims'
leg read every claim, and the starts' leg read every record's labels in `raw_docs` to find the
starts (6,498 among 749,283 records on this PC's store): 113 MB a page.

The change. Each leg takes its own entries past the page's place in the whole's order, at most to
the page's end (`?5 + ?6`), as the imported documents' leg already did. The whole's page is among
them. A repository's claims and starts are filtered in the text that has one, so its claims take
`derivations_repo`. The starts take `raw_docs_start`, which the Fts consumer makes after the
table's other indexes:

- `raw_docs_start (ts, device, seq, repo, session) WHERE kind = 'start'`

A repository's starts never take `raw_docs_repo` (the text names `+d.repo`), which walks each of
the repository's tool calls. No other statement names `kind = 'start'`, so none can take the
index. Its first build reads the table once: 0.06 s on a copy of this PC's store, warm.

On a copy of the claims, derivations, corrections, `raw_docs` and imported documents of this PC's
store, the first read of a page of 50 on a new connection, before and after:

| page | before | after |
|---|---|---|
| every repository, the newest | 113.4 MB, 233 ms | 35.7 MB, 93 ms |
| every repository, the newer side of a mid time | 113.3 MB, 122 ms | 35.7 MB, 42 ms |
| every repository, at offset 500 | 114.8 MB, 217 ms | 37.2 MB, 170 ms |
| every repository, imported documents withheld | 113.2 MB, 158 ms | 35.6 MB, 88 ms |
| the repository with the most | 113.5 MB, 134 ms | 0.4 MB, 2.3 ms |
| a mid-size repository | 113.5 MB, 93 ms | 0.7 MB, 7.3 ms |
| a repository with no entries | 113.2 MB, 88 ms | 0.6 MB, 6.5 ms |

Every repository's page still reads every claim: an index of the claims by their time is for a
later change, with the other readers of `derivations`. On the copy, the statement before and the
statement after returned the same rows in 6,912 cases (2,994 with entries): with the imported
documents and without, either side of a time, with the starts and without, six kinds of
repository, nine places and four sizes of page at four offsets.

Tests: `search::b::tests::a_timelines_legs_read_their_own_page_by_index` (every shape's plan, the
indexes as the consumers make them), `search::b::tests::a_timelines_legs_give_the_wholes_rows`
(the rows of the union before this change, from places among entries of one time from every leg,
pages of 1 to 3 at offsets to the third page).

## The month's spend, from an index of its own

What happened. The month's spend was summed from every call in the ledger. `usd_this_month`,
`embed_usd_this_month`, `reserved_usd_this_month` and budget's newest paid reservation
(`reservation_retry_at`) each read the whole `provider_calls` table: 14.5 MB on this PC's ledger
(159,434 calls). The settings page reads the first two each time it loads. Budget reads the first
before each paid call, and the embedding phase reads the second as it reserves a request, inside
the transaction that holds the ledger's write lock.

The change. `providers_db::open` makes one more index, after the table's other indexes and after
the `usd` column, which a ledger from before that column gains in the same open:

- `provider_calls_spend (ts, role, outcome, usd)`

The four reads seek the month's start in it and read nothing else. No other statement on
`provider_calls` changes its plan: all 25, planned in oboete's SQLite on a copy of the ledger with
the index made last. My Work's daily count keeps `provider_calls_my_work`, and the reads by
provider keep `provider_calls_day`. The settings page opens the ledger read-only and never makes
the index: it reads the table as before until a writer (the worker, curation, the embedding phase)
has opened the ledger once. The first build took 93 ms on a copy of this PC's ledger, and the index
is 5.0 MB.

On a copy of this PC's ledger, the first read on a new connection, before and after:

| read | before | after |
|---|---|---|
| `usd_this_month` | 14.5 MB, 34 ms | 1.5 MB, 5.8 ms |
| `embed_usd_this_month` | 14.5 MB, 9.8 ms | 1.5 MB, 3.0 ms |
| `reserved_usd_this_month` | 14.5 MB, 9.5 ms | 1.5 MB, 2.8 ms |
| budget's newest paid reservation | 14.5 MB, 10.8 ms | 1.5 MB, 3.2 ms |

Tests: `providers_db::tests::the_months_spend_is_read_from_an_index_of_its_own` (a ledger from
before `usd`, the month's sums by role and outcome, and each sum's plan), and
`providers_db::tests::my_works_daily_count_is_its_roles_requests_in_a_rolling_day` (My Work's
count keeps its index).

## #1037: the repository list, from counts the triggers keep

What happened. The repository list (`GET /api/repos`, `view::repos_in`) and its names
(`view::repo_names_in`, which Settings' privacy section reads) counted every repository's claims,
imported documents and records on each read. On a copy of this PC's knowledge.db (4.3 GB, 965
repositories, 2026-11-17), the list read 118.6 MB: 2.6 to 2.9 s on its first read and 0.8 s on the
next. The page reads the list again each time the store's version moves.

The change. knowledge.db keeps the counts in a table of their own, `repo_counts`
(`consumer::repo_counts`). It has a row for each repository and leg (claims, imported documents and
records), with the leg's count and its newest time. Triggers on `derivations`, `imported` and
`raw_docs` change it in the transaction of each row written, whatever code writes the row, and three
more abort an update of a column counted. The Fts consumer's step makes the table and the triggers,
and counts the tables once, when the store does not hold exactly this build's: a new or rebuilt
knowledge.db, an older build's store, or a newer build's. On the copy, that count read 118.6 MB in
0.28 s with the pages in memory. Until it is made, both reads count the tables as before. The
imported documents' leg is read only with the proof that no forget of imported history waits
(decision 109), as before.

On the copy, the list read on a new connection went from 118.6 MB and 0.8 to 2.9 s to 0.1 MB and
1.6 ms, with the same 965 rows in the same order.

Not measured yet: what the triggers add to a drain that makes a knowledge.db new (a rebuild, a
restore, a retention pass, a forget of a claim). A bench of bare inserts on a compact copy came to
about 20 µs a record and 40 µs an imported document: tens of seconds on a drain of this PC's store,
as an upper bound.

Tests: `consumer::repo_counts::tests::the_counts_are_made_again_unless_the_store_holds_this_builds_own`,
`the_counts_stay_the_tables_through_inserts_and_deletes` (400 inserts and deletes of every kind),
`every_lookup_of_the_triggers_reads_an_index`, `an_update_of_a_column_counted_aborts` and
`no_writer_replaces_a_row_counted`; and `view::tests::the_repositories_are_read_from_the_counts_kept`,
`the_repositories_counts_follow_every_writer` and
`a_store_without_this_builds_counts_is_counted_from_its_tables`.
