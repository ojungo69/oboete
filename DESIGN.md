---
name: oboete viewer
description: The local viewer of oboete's memory, compiled into the binary as three files (assets/viewer/index.html, app.css, app.js).
colors:
  ground: "#ffffff"
  surface: "#ffffff"
  surface-2: "#f9f8f7"
  hover: "#211b170d"
  ink: "#2c2c2b"
  muted: "#6f6c67"
  line: "#e6e5e3"
  edge: "#d4d3cf"
  line-strong: "#8f8c87"
  accent: "#105fad"
  accent-hover: "#0d4f8f"
  accent-soft: "#e5f2fc"
  accent-text: "#ffffff"
  focus: "#2383e2"
  red-bg: "#fce9e7"
  red: "#b8322f"
  blue-bg: "#e5f2fc"
  blue: "#105fad"
  green-bg: "#e8f1ec"
  green: "#367252"
  yellow-bg: "#f9f3dc"
  yellow: "#8f6623"
  gray-bg: "#f0efed"
  gray: "#6f6c67"
  purple-bg: "#f3ebf9"
  purple: "#7c4d9a"
typography:
  heading:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "26px"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.02em"
  card-title:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "17px"
    fontWeight: 600
    lineHeight: 1.45
    letterSpacing: "-0.01em"
  group-title:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "15px"
    fontWeight: 600
    lineHeight: 1.4
  body:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.65
  lede:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "14.5px"
    fontWeight: 400
    lineHeight: 1.55
  control:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "14px"
    fontWeight: 500
    lineHeight: "20px"
  label:
    fontFamily: "system-ui, -apple-system, Segoe UI, Hiragino Sans, Yu Gothic UI, Noto Sans JP, sans-serif"
    fontSize: "13px"
    fontWeight: 500
  meta:
    fontFamily: "ui-monospace, SF Mono, Menlo, Cascadia Mono, Consolas, Liberation Mono, monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.5
  tag:
    fontFamily: "ui-monospace, SF Mono, Menlo, Cascadia Mono, Consolas, Liberation Mono, monospace"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: "22px"
    letterSpacing: "0.04em"
  brand:
    fontFamily: "ui-monospace, SF Mono, Menlo, Cascadia Mono, Consolas, Liberation Mono, monospace"
    fontSize: "20px"
    fontWeight: 400
    lineHeight: 1
    letterSpacing: "-0.04em"
rounded:
  s: "4px"
  m: "6px"
  l: "8px"
  xl: "12px"
  pill: "999px"
spacing:
  column: "48rem"
  column-my-work: "82rem"
  gutter: "24px"
  gutter-phone: "16px"
  card-gap: "16px"
  feed-gap: "20px"
  card-pad: "22px 24px 18px"
  record-pad: "18px 24px"
  control-gap: "12px"
  chip-gap: "6px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-text}"
    rounded: "{rounded.m}"
    padding: "7px 14px"
    height: "36px"
  button-primary-hover:
    backgroundColor: "{colors.accent-hover}"
  button-quiet:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.m}"
    padding: "7px 14px"
    height: "36px"
  button-quiet-hover:
    backgroundColor: "{colors.hover}"
  button-small:
    rounded: "{rounded.m}"
    padding: "4px 10px"
    height: "30px"
  button-danger:
    backgroundColor: "transparent"
    textColor: "{colors.red}"
    rounded: "{rounded.m}"
  field:
    backgroundColor: "{colors.ground}"
    textColor: "{colors.ink}"
    rounded: "{rounded.m}"
    padding: "7px 10px"
    height: "36px"
  card:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.l}"
    padding: "22px 24px 18px"
  record-card:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.l}"
    padding: "18px 24px"
  session-row:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.l}"
    padding: "16px 22px"
  tag:
    backgroundColor: "{colors.gray-bg}"
    textColor: "{colors.gray}"
    typography: "{typography.tag}"
    rounded: "{rounded.s}"
    padding: "0 8px"
  tag-blue:
    backgroundColor: "{colors.blue-bg}"
    textColor: "{colors.blue}"
  tag-red:
    backgroundColor: "{colors.red-bg}"
    textColor: "{colors.red}"
  tag-green:
    backgroundColor: "{colors.green-bg}"
    textColor: "{colors.green}"
  tag-yellow:
    backgroundColor: "{colors.yellow-bg}"
    textColor: "{colors.yellow}"
  tag-purple:
    backgroundColor: "{colors.purple-bg}"
    textColor: "{colors.purple}"
  agent-pill:
    backgroundColor: "transparent"
    textColor: "{colors.muted}"
    rounded: "{rounded.pill}"
    padding: "0 8px"
  card-toggle:
    backgroundColor: "{colors.surface-2}"
    textColor: "{colors.muted}"
    typography: "{typography.meta}"
    rounded: "{rounded.s}"
    padding: "2px 10px"
    height: "28px"
  card-toggle-active:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-text}"
  chip:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.m}"
    padding: "2px 10px"
    height: "28px"
  chip-pressed:
    backgroundColor: "{colors.accent-soft}"
    textColor: "{colors.accent}"
  segmented-control:
    backgroundColor: "{colors.surface-2}"
    rounded: "{rounded.l}"
    padding: "3px"
    height: "38px"
  segment:
    backgroundColor: "transparent"
    textColor: "{colors.muted}"
    rounded: "{rounded.m}"
    padding: "0 12px"
    height: "30px"
  segment-pressed:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
  switch:
    backgroundColor: "{colors.line-strong}"
    rounded: "10px"
    width: "34px"
    height: "20px"
  switch-on:
    backgroundColor: "{colors.accent}"
  dialog:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.xl}"
    padding: "28px"
---

# Design System: oboete viewer

The viewer is served by `src/view.rs` from three files compiled into the binary: `assets/viewer/index.html`, `assets/viewer/app.css` and `assets/viewer/app.js`. Every value below is read from those files. The frontmatter holds the light values; the dark values, the shadows, the motion and the breakpoints are in this text and in `.impeccable/design.json`. When this file and `app.css` disagree, `app.css` is what ships; fix whichever is wrong in the same change.

## Overview

**Creative North Star: "claude-mem's viewer in Notion's colours"**

Decision 65 (2026-10-31): the viewer's arrangement follows claude-mem's viewer and its colours are Notion's: a header across the window with the views as a segmented control, one centred column with a line under each list view's heading saying what it holds, every record a bordered card with its kind as a tag and its agent as a pill, settings in groups that fold between rules, a session's summary edged in gold and a prompt in purple; white and near-black grounds, warm greys and one blue for actions and focus, each made darker or lighter where WCAG AA needs it; no cream ground, no orange accent, no gradient and no tinted card. The font stack stays the system's. This file replaces `docs/viewer-design.md`. As built, "no gradient" means no gradient used as colour: a few shapes are painted with flat single-stop gradients because the page may load no image (see Shapes).

The page is a tool kept open beside an agent's work, read often and changed rarely. It is dense but not cramped: one 48rem column (82rem on My Work), 15px text at 1.65, cards 16-20px apart. The interaction that defines it is inside the card: facts and narrative open in place, with no navigation. Settings change with one Save.

Depth is a 1px border, not a shadow and not a fill. In light mode the card and the page are both white, so the edge alone separates them. Shadows exist only on what floats over the page.

**Key Characteristics:**
- A header across the whole window; the views as a segmented control, the open view raised.
- One centred column; a heading, a one-line lede, the view's controls, then the list.
- Records as bordered 8px cards with no shadow and no fill tint.
- A card's kind as an uppercase mono tag in one of six hue pairs; the agent as an outlined pill; ids and times in 12px mono, "#id • time".
- One blue for what acts (primary buttons, pressed toggles, focus); links are underlined ink.
- The system's fonts only, a sans for text and a mono for the brand, tags, ids and times.

## Colors

Two grounds (white and near-black), warm greys for muted text and lines, one blue, and six hue pairs for kinds and states. Tokens are custom properties on `:root`. The dark set is declared twice with the same values: under `@media (prefers-color-scheme: dark)` for `:root:not([data-theme="light"])`, and for `:root[data-theme="dark"]`. `:root[data-theme="light"]` sets only `color-scheme: light`. A change to one dark block is made to both.

Where the values come from: the white and near-black grounds are Notion's. The greys, the blue and the hue families follow Notion's palette; each text colour, `--muted`, `--line-strong` and the accent was darkened (light) or lightened (dark) until the ratios below hold. The ratios were computed with the WCAG 2 formula from the shipped hex values.

### Primary
- **Action Blue** (light `#105fad`, dark `#4fa7ff`): primary buttons, an open facts/narrative toggle, a pressed chip's text and edge, the switch when on, a search heading's query words, "Show more", the "open" label on My Work cards. On the ground 6.44:1 light, 6.95:1 dark. Its hover is `#0d4f8f` light, `#7dbcff` dark.
- **Text on Blue** (light `#ffffff`, dark `#191919`): 6.44:1 on the accent and 8.29:1 on its hover in light; 6.95:1 and 8.8:1 in dark.
- **Blue Wash** (light `#e5f2fc`, dark `#233850`): a pressed chip's fill, My Work's banner, My Focus's change bar, the avatar disc. The accent on it reads 5.65:1 light, 4.74:1 dark.
- **Focus Blue** (`#2383e2`, both themes): every focus outline and a field's focused edge. Notion's blue, unchanged; 3.88:1 against white, 4.53:1 against `#191919`, above the 3:1 non-text minimum. Text selection is the same blue at 28% (`#2383e247`).

### Neutral
- **Ground** (light `#ffffff`, dark `#191919`): the page and the header.
- **Surface** (light `#ffffff`, dark `#252525`): cards, the save bar, the dialog, a pressed segment. In light it equals the ground; the card's edge carries the separation.
- **Quiet Surface** (light `#f9f8f7`, dark `#202020`): the segmented control's track, facts/narrative toggles at rest, the narrative block, details, the context text, disabled fields, the callout behind the start guide.
- **Hover** (light `#211b170d`, dark `#ffffff0e`): translucent, so it also shows on a raised surface. It is the only hover fill for quiet controls.
- **Ink** (light `#2c2c2b`, dark `#f0efed`): text. 13.98:1 on white and 13.18:1 on the quiet surface; 15.3:1 on `#191919`, 13.34:1 on `#252525`.
- **Muted** (light `#6f6c67`, dark `#ada9a3`): lede, meta lines, descriptions, inactive segments. Light: 5.23:1 on white, 4.93:1 on the quiet surface, 4.55:1 on the grey tag fill. Dark: 7.52:1, 6.56:1 and 6.97:1 on the three grounds.
- **Line** (light `#e6e5e3`, dark `#383836`): rules between settings groups and rows, table rules, the header's bottom border, the footer rule of a summary.
- **Edge** (light `#d4d3cf`, dark `#383836`, the same as Line in dark): every card's border and the segmented control's border. Line and Edge are 1.3-1.5:1 and are decoration; nothing relies on them to be seen.
- **Strong Line** (light `#8f8c87`, dark `#75726d`): a field's edge, the switch track when off, the empty state's ring, a hovered card's edge. 3.35:1 and 3.16:1 light, 3.67:1 to 3.2:1 dark, at least 3:1 against every ground it sits on.
- **Scrim** (light `rgb(25 25 25 / 0.45)`, dark `rgb(0 0 0 / 0.6)`): behind the welcome dialog.

### Hue pairs (kinds and states)
Each pair is a fill and a text colour, used together. Dark values in brackets. Ratios are text on its own fill.
- **Red** `#fce9e7` / `#b8322f` (`#502c29` / `#f28b84`): bugfix, security_alert, errors, danger buttons, a forget preview's edge. 5.08:1 (5.08:1).
- **Blue** `#e5f2fc` / `#105fad` (`#233850` / `#4fa7ff`): decision, delivered, quote-only, My Work's "working". 5.65:1 (4.74:1).
- **Green** `#e8f1ec` / `#367252` (`#263d30` / `#7fbf9a`): discovery, feature, change, current, citable, the live dot, "ready". 4.94:1 (5.48:1).
- **Gold** `#f9f3dc` / `#8f6623` (`#504425` / `#e0b45e`): preference, summary, superseded, security_note, sensitive; a session summary's edge, title and labels; warnings. 4.61:1 (4.95:1); gold text on white 5.13:1.
- **Grey** `#f0efed` / `#6f6c67` (`#383836` / `#ada9a3`): every other kind, counts. 4.55:1 (5.03:1).
- **Purple** `#f3ebf9` / `#7c4d9a` (`#3c2d47` / `#b98fd0`): prompt. 5.34:1 (4.77:1).

### Named Rules
**The One Blue Rule.** Only primary buttons, pressed toggles and chips, the on switch and focus use the blue. Links read as underlined ink (underline in Strong Line, in the text colour on hover).

**The No Tint Rule.** No card has a tinted fill. A summary is marked by a gold edge, a dashed gold rule and gold labels; a prompt by a purple edge. Both edges are the hue mixed 55% into the ground (`color-mix(in srgb, var(--yellow) 55%, var(--bg))`).

**The Pair Rule.** A hue's text colour is used on its own fill or on a plain ground, never on another hue's fill. A new kind takes one of the six pairs; it does not add a seventh.

## Typography

**Body Font:** the system's sans (`system-ui, -apple-system, "Segoe UI", "Hiragino Sans", "Yu Gothic UI", "Noto Sans JP", sans-serif`).
**Mono Font:** the system's mono (`ui-monospace, "SF Mono", Menlo, "Cascadia Mono", Consolas, "Liberation Mono", monospace`).

**Character:** no web font can load (see Do's and Don'ts), so the sans carries the page and the mono marks what is machine-made: the brand, kinds, ids, times, counts, repository names.

### Hierarchy
- **Heading** (600, 26px, 1.25, -0.02em, balanced wrap; 22px at 640px or less): the view's heading, one per page.
- **Card title** (600, 17px, 1.45, -0.01em; 16px at 640px or less): a timeline card. A summary's title is 18px in gold. A session row's title and a record's title are 16px. A My Work card's title is 16.5px.
- **Group title** (600, 15px, 1.4): a settings group. Headings inside a group are 14.5px (h3) and 14px (h4); inside stats and details, 15px.
- **Body** (400, 15px, 1.65): text. Card subtitles and narrative are 14px at 1.7. Measure is capped at 72ch for ledes, descriptions and notes.
- **Lede** (400, 14.5px, 1.55, muted): the line under a list view's heading.
- **Control** (500, 14px, 20px line): buttons and fields. Small buttons, segments and labels are 13px.
- **Meta** (mono 400, 12px, 1.5, muted): card header and footer, session meta, the live state, counts. Document and context text is mono 12.5px.
- **Tag** (mono 500, 12px, 22px line, 0.04em, uppercase): a kind. A summary's section label is mono 600, 11.5px, uppercase, 0.04em, in gold.
- **Brand** (mono 400, 20px, -0.04em; 17px at 640px or less): "oboete" beside the mark.

Japanese headings, tags and tabs use `font-feature-settings: "palt"`. Paragraphs, list items and descriptions use `text-wrap: pretty`.

### Named Rules
**The Mono Means Machine Rule.** Mono is for values the system produced (kind, id, time, repository, count). People's words and the page's own text are in the sans.

**The Page's Language Rule.** Times are shown in the page's language (`localTime(ts, lang)`), records included.

## Layout

**Column.** `main` is centred, `max-width: calc(48rem + 2 * var(--gutter))`, padding 32px gutter 96px. My Work widens it to 82rem. The header is not in the column: it spans the window at every width.

**Gutter.** 24px; 16px at 640px or less.

**Header and its views**, measured by `--header-h`, which also sets `scroll-padding-top` and where sticky layers stop:
- 1100px and wider: one row, 65px. Mark and brand, the live state, the eight views as a segmented control, then Theme and the help button pushed right. `--header-h: 65px`.
- 761px to 1099px: two rows, 103px (56px row, 38px control, 8px margin, 1px border). The eight views (about 650px) fit on one line.
- 760px or less: the header is static and scrolls away; the control spans the width and wraps to as many rows as it needs, so no view is out of sight. `--header-h: 0px`.
- 640px or less, in addition: the top row is 52px, the brand 17px, the mark 22px.
- 500px tall or less (any width): the header is static.

**Page head.** Heading, then the lede pulled up 12px under it (`margin: -12px 0 24px`), then the controls (repository, search and filters; flex, 12px gap, aligned to the bottom), then the status line, then the list. The lede is shown on Timeline, Sessions and Records, and not on a session's own page.

**Rhythm.** Feed cards 20px apart (14px at 640px or less); record cards and grid cards 16px; session rows 12px. Inside a card: 12px from header to title, 10px to subtitle, 14px to facts or narrative, 16px to the footer.

**Other breakpoints.**
- 850px or less: My Work's card and agent grids go to one column.
- 768px or less: card padding 18px 20px 16px; records 16px 20px.
- 640px or less: cards 16px 16px 14px, records 14px 16px; a summary's sections stack; stats go to one column; the record filters become a two-column grid; My Work's table becomes labelled blocks; the welcome's rows stack.
- 480px or less: a timeline card's facts/narrative toggles move from the header's right back under the subtitle, in the page's order.

## Elevation & Depth

The system is flat. Cards, tags, rows and groups carry no shadow; a 1px edge or rule separates them. Shadows belong to what floats over the content while it scrolls, to the welcome dialog, and to the raised segment of a segmented control.

### Shadow Vocabulary
- **Raised segment** (`--shadow-s`, light `0 1px 3px #0000001f`, dark `0 1px 3px #00000059`, after a 1px Line ring: `0 0 0 1px var(--line), var(--shadow-s)`): the pressed segment of every segmented control (the header's views, Settings' and My Work's switches).
- **Floating bar** (`--shadow-m`, light `0 8px 12px #19191907, 0 2px 6px #19191907, 0 0 0 1px #2a1c0012`; dark `0 0 0 1px #383836, 0 4px 12px -2px #00000040`): the sticky status line on Settings and on an error, the save bar, My Work's banner.
- **Overlay** (`--shadow-l`, light `0 20px 24px #1919190d, 0 5px 8px #19191907, 0 0 0 1px #2a1c0012`; dark `0 0 0 1px #383836, 0 14px 28px -6px #00000059, 0 2px 4px -1px #0000003d`): the welcome dialog and My Focus's menu.

### Layers
The header 10; the sticky status line, the save bar, My Work's banner and My Focus's menu 5; the welcome 50. A My Focus chip inside a My Work card is raised to 1 so it sits above the card's stretched opener.

### Named Rules
**The Flat Card Rule.** A card is a border, never a shadow. If something needs to look raised, it is floating over scrolled content, or it is the pressed segment.

## Shapes

Gently rounded rectangles on one scale: 4px (`--radius-s`) for tags, facts/narrative toggles and counts; 6px (`--radius-m`) for buttons, fields, segments, chips and inner panels (narrative, details); 8px (`--radius-l`) for cards, rows, callouts, the segmented track and floating bars; 12px (`--radius-xl`) for the welcome dialog. The agent pill is fully round (999px); the switch is a 34x20px capsule. The mark is an inline SVG: a tile (rx 9 on 32) in ink, a ring and a dot in the ground colour.

Shapes the page draws without images are made from borders and flat single-stop gradients: chevrons are two-sided borders rotated 45deg; the live dot and the "new" dot are 6px circles; the empty state is a 32px ring in Strong Line; the loading skeleton is four flat Grey Fill bars painted with `linear-gradient(var(--gray-bg) 0 0)`; the switch's knob is a 16px `radial-gradient` circle in the ground colour (in Text on Blue when on).

## Components

### Header
- **Anatomy:** the mark (24px) and "oboete" in mono; the live state (6px dot and a mono word, ellipsised when the row is short); the views; Theme (quiet small button, "Theme: auto/light/dark") and help ("?", 32x32px).
- **Live state:** green dot and muted text; with class `off`, both red. The dot breathes (opacity to 0.35 over 2.4s) unless reduced motion.
- **Behaviour per width:** see Layout.

### Segmented control
Used three times: the header's views (`#tabs`, buttons `.tab`, the open one `.active`), Settings' simple and detailed views (`.settings-view-switch`, `aria-pressed`), My Work's cards, list and agents (`.my-work-switch`, `aria-pressed`).
- **Track:** Quiet Surface, 1px Edge, 8px radius, 3px padding, 2px gap; the header's is 38px high.
- **Segment:** 30px, 6px radius, no border, muted 13px weight 500; hover takes the Hover fill and ink.
- **Pressed:** raised: Surface fill, ink, the raised-segment shadow with a 1px Line ring.
- **Focus:** the focus outline inset by 2px inside the header's track.

### Page head and lede
The heading (26px), a search's query words in the accent, then the lede in muted 14.5px, at most 72ch.

### Timeline card (observation)
- **Anatomy:** a header row (the kind tag, the agent pill, the repository in mono) with the facts and narrative toggles at its right; the title; the subtitle; the open body; the footer.
- **Container:** Surface, 1px Edge, 8px radius, padding 22px 24px 18px; no shadow.
- **Open body (the signature interaction):** "facts" swaps in a list with muted markers, concept tags (sentence case, no letter spacing) and file lists in mono; "narrative" swaps in a Quiet Surface block, 6px radius, 12px 16px padding, up to 300px tall and scrolling. The card body changes in place.
- **Footer:** mono 12px muted: `#id • time` (the "#" and "•" are drawn by CSS and hidden from screen readers), then any control pushed to the right as a borderless quiet button in sans 12.5px.

### Facts / narrative toggles
28px high, 2px 10px, 4px radius, Quiet Surface with an Edge border, muted mono 12px. Hover: Grey Fill, Strong Line border, ink. Open: the accent fill, no border, Text on Blue.

### Type tag and agent pill
- **Tag:** a hue pair's fill and text, 0 8px, 4px radius, mono 500 12px on a 22px line, uppercase, 0.04em. In a meta line, a second tag shows its hue as plain text without a fill.
- **Agent pill:** transparent, 1px Edge, fully round, muted mono 600 uppercase on a 20px line (22px high with its border). The agent is never a second filled tag.

### Summary card and prompt card
- **Summary:** the card with a gold edge; its header row has a dashed gold rule under it; the title is 18px gold; its sections sit two to a row (one at 640px or less), each with a gold mono uppercase label and 14px text; the footer has a Line rule above it.
- **Prompt:** the card with a purple edge; the person's text whole, clamped to eight lines behind "Show more" (accent text, no fill, underlined on hover).
- Neither is filled.

### Session rows and records
- **Session row:** a full-width button styled as a card: Surface, Edge, 8px radius, 16px 22px; title 16px 600, then a mono meta line (agent pill, time, id). Hover: Strong Line edge, no fill change.
- **Session chips:** 28px, 2px 10px, 12.5px quiet small buttons; pressed (`aria-pressed="true"`): accent edge, Blue Wash fill, accent text.
- **Record card** (Records view): Surface, Edge, 8px radius, 18px 24px; a mono meta line of tags, time and key; a 16px title; a delivered record is indented 24px (12px on a phone). Buttons inside a record are quiet and small.
- **Detail:** a claim or document opens under what opened it, on Quiet Surface, 6px radius, 14px 16px.

### Settings
- **Language** row first, then groups.
- **Group:** a `details` between 1px Line rules, no box. Its summary is a grid: title (15px 600) and description (13px muted) at the left, a chevron (7px, 1.5px muted borders) at the right that turns from down to up when open (150ms). Sections inside a group are separated by Line rules with 20px above.
- **Start guide:** the one group shown as a callout: Quiet Surface, no rules, 8px radius, 0 20px padding.
- **Smaller disclosures:** inline, 14px 500, a 6px chevron before the words.
- **Save bar:** sticky 12px from the bottom, Surface, 8px radius, the floating-bar shadow, Save at the right with a 10rem minimum.
- **Warnings** after the form: Gold Fill callout, 8px radius.

### Fields
36px high, 7px 10px, 6px radius, the ground as fill, a Strong Line edge. Hover: the edge in Muted. Focus: the edge in Focus Blue doubled by a 1px ring of the same blue. Invalid (`.invalid`): the edge in red, and a red ring when focused. Disabled: Quiet Surface fill, muted text. Placeholders in muted at full opacity. Checkboxes and radios are 16px with the accent as their colour. Labels sit above in 13px 500 with a 6px gap.

### Buttons
- **Primary:** accent fill, Text on Blue, 36px, 7px 14px, 6px radius, 500. Hover: Accent Hover.
- **Quiet:** transparent, ink, Edge border. Hover: Hover fill and Strong Line border.
- **Small:** 30px, 4px 10px, 13px.
- **Danger:** red text and red border; hover takes the Hover fill.
- **Disabled:** 50% opacity, `not-allowed` cursor.
- **Focus (all):** a 2px Focus Blue outline, 2px offset.

### Switch
A checkbox with `role="switch"`, drawn with `appearance: none`: a 34x20px capsule in Strong Line with a 16px knob at 2px. On: the accent capsule, the knob in Text on Blue, moved to 16px. Disabled: 50% opacity. Focus: the standard outline. In a toggle row the words come first and the switch sits at the right (max 32rem).

### Callouts
Flat, 8px radius, 12px 16px, no border: Gold Fill for warnings and the vector-search note, Blue Wash for an empty simple list and My Work's banner, Quiet Surface for consent notes and add panels. A forget preview is the exception: Surface with a red edge, because it asks before a destructive act.

### Status line
`#status` under the controls, 14px muted; `.error` in red. On Settings and for any error it becomes a floating bar, sticky under the header (`top: calc(var(--header-h) + 10px)`), Surface, 8px radius, the floating-bar shadow; an error adds a red edge.

### Empty, loading, error
Inside `.feed-state` under an empty list:
- **Empty:** 56px 24px padding, the 32px Strong Line ring above a muted centred line.
- **Loading** (`.feed-state.loading`): a card-shaped block (Surface, Edge, 8px radius, 22px 24px) holding four flat grey bars (6rem by 20px, 70% by 16px, 92% by 12px, 55% by 12px) that pulse to 50% opacity over 1.4s unless reduced motion.
- **Error:** red text, 40px 24px 4px, no ring.
- **End of the list:** the words in mono 12px between two Line rules.

### My Work
- **Switch:** the segmented control above.
- **Toolbar:** search and filters in a wrapping row, 8px gap.
- **Status tabs:** a row of borderless tabs on a Line rule; the chosen one is underlined 2px in ink; each carries a count in a mono 11.5px grey box.
- **My Focus chips:** the same size as the session chips (28px, 2px 10px, 12.5px), pressed the same way.
- **Cards:** two to a row (one at 850px or less), Surface, Edge, 8px radius, 20px 22px; the title is a button whose stretched area covers the card; focus shows on the whole card (`:focus-within`, Focus Blue). Hover: Strong Line edge. A footer under a Line rule holds the next step and the accent "open" label. An updated card's fill and edge fade from Blue Wash and the accent once (1.4s) unless reduced motion.
- **List:** a bordered table; at 640px or less each row becomes a block with the column's label above each value.

### Welcome
A 44rem dialog, Surface, 12px radius, 28px padding (20px on a phone), the overlay shadow, over the scrim. Its parts are rows: a 9rem title column and the text, between Line rules.

## Do's and Don'ts

### Do:
- **Do** take every colour, radius and shadow from the custom properties on `:root`, and set a new dark value in both dark blocks.
- **Do** keep text at 4.5:1 or more and field edges, the off switch and focus at 3:1 or more against what is behind them, in both themes.
- **Do** separate a card from the page with a 1px Edge border; in light mode it is the only thing that does.
- **Do** give a new kind one of the six hue pairs and use it in every list the kind appears in.
- **Do** keep motion to 150ms ease transitions and wrap any keyframe animation in `@media (prefers-reduced-motion: no-preference)`. Transitions today animate colour, background, border, the chevrons' rotation and the switch knob's position; `prefers-reduced-motion: reduce` turns all transitions off.
- **Do** draw shapes with borders, circles and flat single-stop gradients. The CSP (`default-src 'none'; style-src 'self'; img-src 'self'; script-src 'self'`, set in `src/view.rs`) allows no web font, no `data:` image, no inline style and no inline script, so a font file, an icon image, a `style` attribute or a `<style>` block does not load.
- **Do** check every view in English and Japanese, light and dark, at 1280px and 360px, with settings groups open, before changing the CSS.

### Don't:
- **Don't** add a shadow to a card, a tag, a row or a group. Shadows are for the pressed segment, floating bars, the menu and the dialog.
- **Don't** fill a card with a tint, use a cream ground or an orange accent, or use a gradient as colour (a fade or a two-colour blend).
- **Don't** colour links with the accent; links are underlined ink.
- **Don't** add a web font or rely on a specific font's metrics; the stacks fall back to whatever the system has.
- **Don't** rename or reorder classes, ids, `data-*` attributes or texts that `src/testdata/viewer-readiness/test.mjs` reads. Its small fake DOM finds elements by tag name and by class only, so a style change must be made in the CSS, not by changing markup. Some checks compare the whole `className`: `my-work-card ` must stay the first class of a My Work card, and `my-work-heading`, `my-work-stream` and `show-more` are matched exactly. It reads the `data-` attributes `action`, `field`, `focus`, `i`, `id`, `kind`, `my-work`, `provider-name`, `session`, `simple-block`, `simple-focus` and `time`.
- **Don't** add a label above a heading (an "eyebrow" or kicker), a glyph used as an icon, or a hard offset shadow. The page draws its marks with borders and SVG.
