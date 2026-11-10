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
`worker::tests::the_repository_index_is_built_in_a_pause_beside_the_session_index`.

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

That is not nothing left behind. Measured after the merge, on this PC's store, a thread a read as
the viewer runs them: after 40 reads of My Work the process held 174 MB with a `Warm` and 161 MB
without one, and neither grew over the next 20. A `Warm` adds 12 to 13 MB to where the process's
memory settles. Most of what settles there is not the `Warm`'s: counting anonymous memory alone,
a thread a read left 152 MB after 40 reads and the same 40 reads on one thread left 37 MB (#922).

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
