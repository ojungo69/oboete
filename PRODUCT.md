# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Developers who run several AI coding agents (Claude Code, Codex, Grok Build and others) on their
own machine, across many sessions and repositories. Each agent starts every session without what
the last one did, and these developers do not want to repeat it. Today the maintainer uses oboete
every day on Linux (WSL). After the public release, other developers install it on Linux, macOS
and Windows.

## Product Purpose

oboete is one Rust binary. It records what coding agents do through each agent's hooks: prompts,
replies, tool calls and their results. It keeps the record in SQLite files on the user's own disk,
and hands the next session what the last one left: the last exchange, the last failing command,
risky git state, the files touched, the user's directives and the open to-do lists. The record can
be searched from the terminal, from the agent over MCP, and on a local web page. Optional AI
summaries turn the record into what was learned (「覚えたこと」 on the Japanese page, "Learned" on
the English one) and into summaries of sessions and of work.

Success: an agent, of any supported kind, starts where the last session stopped, and the user does
not explain the work again. A user can set up, configure and manage oboete from the local page alone.

## Positioning

oboete is to be a complete superset of claude-mem, the closest neighbor, and to go beyond it. It
keeps following claude-mem's releases. Its claims:

- **One record across agents.** One store takes every agent's sessions, so the next session in one
  agent starts from what another did (Claude Code's work picked up by Codex).
- **It stays on the machine.** Nothing is sent anywhere until the user turns on AI summaries or an
  online embedder. Secrets are masked before they are stored. The user can forget a repository, a
  session or a record, pause capture, and keep paths out.
- **It is light.** One binary, with no separate database server or vector-store process.
- **Everything claude-mem does, and measurably more.** Where oboete claims to be better (search
  accuracy, speed, memory), the claim rests on a measurement against claude-mem on the same data.
  No claim of being ahead is made without one.

## Operating Context

- The agents' hooks run oboete on each prompt, tool call and session start. A hook never fails the
  agent: it waits at most two seconds for the store, and keeps the event in a file for a later hook
  when the store is busy or the write fails.
- A resident worker indexes, embeds and summarizes in the background, and serves the page
  (`oboete view --open`) on localhost.
- The page's views: the timeline, records, sessions, My Work, context, stats and settings. The
  timeline and records, and settings, are the most used.
- The light and dark themes are both in daily use, about equally.
- Contract for the page (docs/spec-webui.md, 2026-10-02, #94): after installing and launching
  oboete, a user completes setup, every user-configurable setting and every supported management
  operation on the page, with no shell command and no edit of a config file. The CLI stays for
  automation and recovery.
- A user of claude-mem can import its history (README, "Coming from claude-mem").

## Capabilities and Constraints

- Agents: Claude Code, Codex and Grok Build are in daily use. Adapters for Cursor, OpenCode,
  Antigravity (agy) and Pi are built and tested, and not yet used in live sessions. The aim is
  every agent claude-mem supports, at the same depth or deeper (capture, context and search), and
  agents it does not support.
- AI summaries run through providers the user turns on, each with its own key. Paid use stops at
  monthly caps the user sets, and costs are shown on the page.
- Search by meaning is optional: a local model (bge-m3) or an online embedder.
- Stores: SQLite files under `~/.oboete` (raw.db, knowledge.db, providers.db).
- Pre-release: oboete is built from source, there is no tagged release yet, and its file formats
  may still change. Trial builds may be published as GitHub pre-releases (decision 92,
  2026-11-10).
- Undecided: whether the page becomes claude-mem's own viewer, served by oboete with oboete's data
  (a throwaway spike checks this first), or stays oboete's page, extended by hand to match it.

## Brand Commitments

- Name: oboete, in lower case (覚えて, Japanese for "remember this").
- Tagline (README): "Your coding agents forget between sessions. oboete remembers, on your
  machine."
- Assets: the logo, the symbol and the favicons in `assets/brand/`, each with a dark and a light
  variant. The page shows the variant that matches its own theme setting.
- Visual reference (2026-11-17): the page follows claude-mem's viewer as it is, with oboete's name
  and logo in place of claude-mem's and a Japanese font added. To avoid: the colors typical of
  AI-made sites, rounded cards and badges everywhere, and crowded screens.
- Voice: plain and factual. Figures are measured, with where they come from, and never estimated
  in public copy. On-screen text is polite, natural Japanese and English, without abbreviations or
  jargon.

## Evidence on Hand

- Measurements: README's "Measured" table (hook latency, the session-start hook against
  claude-mem's, the resident worker's memory), `docs/page-reads.md` (what the page's reads cost),
  `docs/resident.md`, the milestone documents, and the parity table against claude-mem
  (`docs/claude-mem-parity.md`).
- Screenshots: `docs/images/timeline-light.png`, `docs/images/timeline-dark.png`.
- Brand kit: `assets/brand/`.
- Absent, never to be invented: a tagged release, download or star figures, user testimonials,
  third-party reviews or press, and any comparison with claude-mem that was not measured.

## Product Principles

1. A record is never lost or leaked: a hook does not fail the agent, secrets are masked before they
   are stored, and a forget reaches every copy.
2. Local first: nothing leaves the machine until the user turns it on, and a cost is shown and
   capped before it is spent.
3. Measure before claiming: speed, accuracy and memory claims rest on measurements, against
   claude-mem where they compare.
4. Everything on the page: setup, settings and management work from the local page, without a
   terminal.
5. Follow claude-mem, then go beyond it: whatever claude-mem's users rely on is matched before
   anything is claimed beyond it.

## Accessibility & Inclusion

- Japanese and English are equal: every screen and every message exists in both.
- Japanese input (IME) and CJK text must work on the page.
- Both themes, light and dark, are complete.
