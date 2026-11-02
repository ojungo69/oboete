# The settings page, folded as claude-mem's (decision 69)

Decision 69 (2026-11-02) makes the settings page as quick to read as claude-mem's settings, with nothing it can do taken away. This note records what was measured, what changes, and the order of the work. [spec-webui.md](spec-webui.md) still holds: every setting has a path in the page, both languages, a confirmation before a send or a removal, keys written and never shown, and the four no-cap switches of decision 64.

## What was measured (2026-11-02)

- **oboete, simple settings, 1280 px wide, this PC's store**: one page, 5,296 px tall, its eight sections all open: 21 inputs, 9 selects and 67 buttons.
  - The AI for summaries were ten entries drawn in full, about 2,290 px, whether in use or not.
  - The places never recorded were one framed row with its own button each, about 1,290 px.
- **claude-mem 15.8's settings** (read from `ui/viewer-bundle.js`, its worker too busy to draw the page that morning): one dialog, 1,200 px wide.
  - On its left, the context the next session receives.
  - On its right, four sections. Loading and Display open; Backups and Advanced closed.
  - Advanced holds the provider, and shows only the chosen provider's fields.

## What changes

1. **The groups fold.** The simple page is groups that fold, as the detailed page's are (`settingsGroup`). What agents are given and the AI for summaries open first; My Work written by AI, Search by meaning, Privacy (the places never recorded, masking, what was removed) and Agents start closed. A closed group's heading says what it holds now, so it need not be opened to be read: "At session start: on · With each prompt: off", "2 in use, tried in this order · $0.39 used this month", "3 places never recorded · Masking of secrets: on". A group stays as the owner left it through the redraws a poll or a save brings.
2. **One line for each AI.** The AI in use are numbered lines in the order they are tried: name, model, state. A line opens on what to change (its switch, its model, its key, a test, its place). One that needs the owner (a key to enter, a rest only the owner ends) opens by itself. The AI turned off fold together under "AI not in use (n)". The arrows move an AI past the ones turned off, to the next in use.
3. **The places never recorded fold** under their count; adding a place stays outside the fold.
4. **The month's spend in cents** ($0.39), under a cent as "<$0.01", where the page said $0.385626.
5. **One page, no Simple and Detailed switch** (a later slice). Each AI line's "detailed settings" opens its entry's editor in place (base URL, timeout, calls a day, limits), and the detailed page's other parts move into the groups they belong to, so the switch goes. **Each change saves at once** (decision 69, 2026-11-02), as the simple page saves now and unlike claude-mem's one Save: a switch or a choice when it changes, a number or a text when it is left or Enter is pressed. Adding, editing or removing an AI entry keeps its own button, and what sends or removes data keeps its confirmation. The page's port moves the page, so it keeps a button of its own with a note of where the page goes.
6. **What the next session receives, behind a button** (a later slice): "See what is given" in the first group shows the Context view's text, where claude-mem shows it beside its settings.

## Slices

| Slice | What | Acceptance tests |
|---|---|---|
| 69a | Changes 1 to 4 | The groups in order, two open and four closed, each closed heading's state in both languages; the AI in use numbered, the AI turned off folded with their count; an arrow skips an AI turned off; the places folded under their count; the spend in cents; a group opened stays open through a redraw. |
| 69b-1 | The detailed page saves each change at once: its Save bar goes; each control of its general form commits the whole form as its Save did, with the version check and the field named when it is refused | Each kind of control saves on its trigger and only then; an invalid number is marked and nothing is sent; a refused save names its field; the port changes only with its button, after the same confirmations as any save; saves go one at a time, a change made during one sent after its answer with its version; what is typed during a save stays, a port typed and not moved to included, and the focus goes back where it went, its fold opened; the changes waiting for a confirmation are said and confirmed together; a hash is saved once it is made; the start guide points at no Save button. |
| 69b-2 | One page: each group gains a "More settings" fold holding the detailed controls that belong to it (the table below), and the groups This computer (resident worker, page port, backups) and Maintenance (the page's address and token, what was removed, checks, update); the switch and the two views' duplicate sections go | Every control of the detailed page is reachable on the one page, each once; both languages; no switch. |
| 69b-3 | Each AI line opens its entry's editor (base URL, model, key, calls a day and no cap, timeout, limits, remove) and says when a rest ends; issue #800 item 3 | A line's edit saves at once and reaches the next pass; a resting AI says until when; a subscription entry with a finite cap shows its switch. |
| 69c | Change 6 | The button shows what the Context view shows, and calls no model. |

Issue #800's subscription cap switch (item 3) belongs to 69b-3; the privacy group's 0.8 s read of raw.db and #794's test wording to 69b-2.

## Where the detailed page's controls go (69b-2)

| Control (detailed page) | Group | Saves |
|---|---|---|
| Summary language, window size, idle minutes, Gemini's place | AI for summaries, More settings | on leaving the field; Gemini on change |
| Each entry's calls a day and no cap, timeout, model | its AI line (69b-3) | on leaving the field; no cap with its warning (decision 64), no confirmation |
| Adding, editing, removing an entry; a key | AI for summaries and its AI line | own buttons, as now |
| A stopped entry's resume | its AI line | own button, as now |
| Session start's note and list, memory lookup, memory instructions, correction, the four sizes | What agents are given, More settings | switches on change, sizes on leaving the field |
| My Work's calls a day and no cap; the judge (switch, destination, key, calls a day, test) | My Work | as above; the judge's key and test keep their buttons |
| Workers AI's account, requests a day, dollars a month | Search by meaning, More settings | on leaving the field |
| Masking, prompts kept, tool output kept, extra masking rules and exceptions, retention, every repository's and pattern's exclusion | Privacy, More settings | switches and choices on change, texts on leaving the field; masking off and a saved rule removed after their confirmation; a new rule once it has its id and pattern; a hash once it is made; exclusions with their buttons |
| Preferences, the agents' detailed setup | Agents | as now |
| Resident worker, page port, backup folder | This computer | resident on change, backup folder on leaving the field, port with its button |
| The page's address and token, what was removed, checks, update | Maintenance | own buttons and confirmations, as now |
| Start guide, a config.toml error's recovery, warnings | at the top, as now | as now |
