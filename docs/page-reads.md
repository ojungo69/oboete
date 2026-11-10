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
  refresh does not end there, and reads the view for what the picker says.

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
  moved with no read behind it does not end the poll;
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

The change is to the second. `view::repo_names_in` reads the names alone, from the same three
tables under the same filters, each from an index on its repository: 0.14 s the first time and
again. The page's rows are the same.

Measured, and not changed:

- Each name is checked against the redaction rules three or four times for a row (what a hook
  would store it as, twice or more, and what the page shows). On a made-up store of 950
  repositories and 3,800 records the whole request takes 13 to 37 ms on a release build, the
  checks included, so a check kept and used again would save a few milliseconds. #899's steps 3
  and 4 are not built.

Not in this change:

- The read of every record of raw.db. It is what finds the repositories knowledge.db does not
  name: those of records not yet indexed, and those of another device's records, which the index
  never reads. It comes next, with tests that hold each of the two.

Tests: `view::tests::the_repositories_names_are_the_lists_names_read_from_indexes` (the names
are the list's, a repository in one table alone too, no name twice, a record of no repository no
name, each table read from an index) and
`settings::privacy::tests::the_names_only_knowledge_holds_are_listed_and_masked_as_shown` (a name
only knowledge.db holds is listed, one a rule masks is shown masked under the selector of the
name as stored, and no repository is counted for the page).
