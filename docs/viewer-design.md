# The viewer's look

On 2026-10-09 the owner said the page is too plain ("簡素すぎる") and that they dislike its design, and asked that oboete beat claude-mem's weakest points, complex settings and low extensibility. This note records the design the page follows since then: what changed and why, its tokens, and what is left for later. It changes how the page looks and is laid out, not what it does: the API, the views' behaviour, the texts' meaning and the page's security rules (spec 6.6, [spec-webui.md](spec-webui.md)) are unchanged.

## Direction

A calm page that looks finished, for an owner who keeps it open while working and who reads Japanese first:

- one centred column (48rem) under a header aligned to it, so every view starts at the same left edge;
- one accent, 藍色 (ai-iro, `#165e83`), for everything the owner can act on: buttons, the active tab, focus, links, an open toggle;
- cards where a thing is a thing (a card, a session summary, a prompt, a record, a stats table, a settings group), plain text elsewhere;
- a colour per kind, the same in every list, so a decision or a bugfix is found by eye;
- the common settings first and open, the rest folded under a one-line description.

The two skills consulted (redesign-existing-projects, design-taste-frontend) are written for landing pages; what carried over is the single accent, hover and press states, one radius scale and states for empty and loading. Their font, image and motion advice does not apply: the CSP allows no web font and no image, and a tool page wants little motion.

## What changed

**Header.** A mark (a ring with a centred dot, the "o" of oboete holding what it remembers) beside the name, the live state as a small pill, theme and help on the right, the tabs on a row below with an accent underline. Only this stays at the top while scrolling (99 px; it scrolls away on screens under 500 px tall). The mark is inline SVG in `index.html`, coloured by CSS classes.

**The view's own tools.** The repository select, the search box and the filters moved from the header into the page, under the view's title. The header had grown to over a quarter of the screen on Records; now it is two short rows on every view.

**Title.** Each view's heading is a real title (24 px, sentence case) instead of a small upper-case label.

**Timeline.** Cards keep claude-mem's parts (page.md P6 to P8) with more hierarchy: type and agent as badges, the title larger, the footer under a hairline. A prompt, the owner's own words, is quieter than a curated card: no shadow, the page's colour, a quote bar. A session summary lays its four parts two by two. Three states are drawn: the first page loading shows the shape of a card (the page now marks its state `loading`), an empty feed shows a composed empty state, and the end of the feed is a quiet rule.

**Records.** Each entry is a card with its badges first; a delivered decision keeps its indent and its blue edge. An opened document or claim is a panel inside the card.

**Context and Stats.** The facts about the checkout sit in a card above the text handed over. Stats shows its tables as cards, two to a row where there is room.

**Settings.** The page was one column of some twenty sections at one weight, 9,000 px long at 1280 px wide. It is now a language picker, the introduction and seven groups, each a card that folds:

| Group | Holds | Open when |
|---|---|---|
| Getting started | the AI tier guide and the test phrase | the home is new |
| Everyday settings | summaries, the paid-call limit, what agents are given, what is recorded | always, by default |
| AI providers | the provider entries, the name controls, providers waiting | a provider waits for the owner |
| Privacy | masking, repositories not sent out | opened |
| This computer | running between sessions, the page's port, backups | the home is new (its save applies the resident choice shown there) |
| Agents | the global preference, agent registrations, connecting agents | opened |
| Diagnostics and maintenance | diagnostics, history and recovery, the page's token | opened |

A group remembers whether it is open across the page's redraws, as a provider's editor already did. Nothing reachable before is hidden for good: the guide's buttons open the group they point into before scrolling to it, a refused field opens the groups around it (as `markInvalid` already did for nested disclosures), and copying a tier opens the providers group whose on/off it changed. The first five groups are inside the settings form, as their fields were; the last two keep their own buttons outside it. "Save settings" sits in a bar that stays at the bottom of the screen while the form is in view, and a save's answer, like any failure on any view, stays at the top of the screen instead of scrolling away.

**Welcome.** The three parts read as rows (title, then text) under the mark, over a dimmed page.

## Tokens

All colours are tokens on `:root`, redefined in the two dark blocks (the system's dark unless the page is forced light, and forced dark); the two dark blocks are identical.

| Token | Light | Dark | Use |
|---|---|---|---|
| `--bg` | `#f6f6f3` | `#141618` | page |
| `--surface` | `#ffffff` | `#1b1e21` | header, cards, fields |
| `--surface-2` | `#efefeb` | `#23272b` | hover, quiet panels |
| `--text` | `#1f2328` | `#e7e8e9` | text |
| `--muted` | `#5d6268` | `#a2a8ae` | secondary text (5:1 or more on every surface) |
| `--line` | `#e3e3de` | `#2c3136` | hairlines, card edges |
| `--line-strong` | `#8a8f95` | `#6f767d` | a field's edge (3:1 or more against its surface) |
| `--accent` | `#165e83` | `#8ec5e6` | the one accent |
| `--accent-hover` | `#0f4d6d` | `#a9d4ee` | a button under the pointer |
| `--accent-soft` | `#e2eef5` | `#1b3343` | an open toggle, the guide's group |
| `--accent-text` | `#ffffff` | `#0d1820` | text on the accent (7:1 and 9.7:1) |
| `--red`, `--blue`, `--green`, `--yellow`, `--gray` (each with `-bg`) | | | kinds and states; each pair 5.3:1 or more |

Kinds: decision, delivered and quote-only blue; bugfix and security_alert red; feature, discovery, change, current and citable green; preference, summary, superseded, security_note and sensitive yellow; the rest gray. An agent and a concept are outlined, not filled.

Radius: 6 px badges, 8 px fields and buttons, 12 px cards and groups, 16 px the dialog. Layers: the header 10, the sticky status and save bar 5, the welcome 50. Motion: 150 ms colour changes, the live dot breathing and the loading shape pulsing; none of it under `prefers-reduced-motion`. Type: the system's fonts (the CSP allows no other), 15 px text, 24 px titles, `palt` for Japanese headings and badges.

## Checked

- `src/testdata/viewer-readiness/test.mjs` passes unchanged: the group wrappers keep every class and `data-*` the page and the test use, and the settings form is still the panel's first form.
- In Chromium through Playwright, every view in English and Japanese, light and dark, at 1280 px and 360 px: no horizontal scroll, and no text below 4.5:1 against what is behind it (badges, notes, descriptions, tabs, buttons; disabled controls excepted), with every settings group and disclosure open.
- Forced dark and the system's dark give the same tokens, as do forced light and the system's light.

## Left for later

- Icons for the tabs and kinds: claude-mem's are its own files, and drawing a set by hand would need its own review.
- A favicon: the viewer serves three files; a fourth needs a change in `src/view.rs`.
- Marking a folded group that holds unsaved changes. Today the guide opens the groups it changes; a typed change in a group the owner then folds is still saved by "Save settings".
- Sessions (page.md slice 2) and a theme menu in place of the theme button's cycle.
- The type was judged on Linux, where `system-ui` is DejaVu Sans; the owner's Windows shows Segoe UI and Yu Gothic UI, which are narrower and may want the sizes looked at again.
