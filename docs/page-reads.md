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
  it again beside its own read (`show`), so a repository made meanwhile is in the picker when the
  picker is next used.
- A list that fails is shown as before, and a read still on its way draws nothing over the
  error. The poll asks the list again each time, as before, and the list alone: the view is not
  read again every 3 seconds while the list fails (`reposFailed`).
- W6: `refresh` notes the page's count of reads (`generation`) as it begins. If a click or a
  search moved it while the list was read, the poll's refresh ends there: the view was read after
  the version the poll saw. Not under My Work, whose way back from a request moves the count
  with no read behind it (`routeShow`).

Unchanged: Settings waits for the list as before (it keeps unsaved drafts and the note of a
config.toml mistake by it); the Refresh button, a forget and a restore read the list under any
view; a view that reads the list has it read live at each change; every answer still passes the
viewer's masking gate (`view::answered`).

## Not in this change

- The list itself is still counted on each read (a cache on the viewer would need a key that
  moves with every table it counts). The page now asks for it far less often.
- My Work's and the settings page's own cost (W1, W2) and the timeline's (W4) are the viewer's
  and come next (#899, with what each was measured at).
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
- a click while a poll reads the list: the view is read once.
