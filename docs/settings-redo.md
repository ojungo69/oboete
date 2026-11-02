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
5. **One page, no Simple and Detailed switch** (a later slice). Each AI line's "detailed settings" opens its entry's editor in place (base URL, timeout, calls a day, limits), and the detailed page's other parts move into the groups they belong to, so the switch goes. The detailed page's single Save and the simple page's save on each change become one rule before the switch is removed.
6. **What the next session receives, behind a button** (a later slice): "See what is given" in the first group shows the Context view's text, where claude-mem shows it beside its settings.

## Slices

| Slice | What | Acceptance tests |
|---|---|---|
| 69a | Changes 1 to 4 | The groups in order, two open and four closed, each closed heading's state in both languages; the AI in use numbered, the AI turned off folded with their count; an arrow skips an AI turned off; the places folded under their count; the spend in cents; a group opened stays open through a redraw. |
| 69b | Change 5 | Every control of the detailed page reachable from the one page; a save keeps a draft the owner has not saved elsewhere; no switch. |
| 69c | Change 6 | The button shows what the Context view shows, and calls no model. |

Issue #800's subscription cap switch (item 3), the privacy group's 0.8 s read of raw.db and #794's test wording belong to 69b.
