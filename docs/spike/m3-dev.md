# Spike: M3 and M2 on the dev transcripts, and what the curator is sent (2026-09-28)

Milestone 3, Task 13 (lines M2, M3, Window, Cost) and Task 12 (judge role (a), "shrink, never drop"). Dev only: no held-out transcript is read. The harness is `docs/eval/m3.py`; its homes and outputs are owner-only, under `~/.oboete/eval/m3/`.

## The dev set

- The sessions: the replay set's 30 dev transcripts, plus every session a dev label is in (the 59 decisions and the 63 pairs of docs/milestone-1.md, "Dev labels"). That makes 63 sessions, 44 of them holding a label. The union is used because 50 of the 59 decisions come from the 40 `dev-extra` transcripts, not from the replay set.
- Each transcript becomes a fixture through the measured binary's own `oboete transcript`.
- All 63 fixtures are merged in time order (event time, then session, then line) and replayed into one home. So a later session's claim can supersede an earlier session's claim (31 of the 63 pairs are cross-session), and the windows see the sessions interleaved as the hooks would have. There are 70,904 events in all.
- The binary is a release build of main at 448085f (sha256 `7b675ef876df`), run by its path. The owner's installed `oboete` is v1 and is never used here.

## Labels to records (`m3.py map`)

- Each labeled decision, and each draft decision a pair names (134 items), is matched to the records of its session that hold its quote. If none does, the first 12 characters of the quote are used instead. Records that repeat earlier text (a compaction summary, a subagent's envelope) are left out. Of the matching records, the one nearest the label's time is taken.
- Result: 132 items match on the whole quote, 1 on its prefix, and 1 on nothing (`d424`, left out of every count with that reason).
- Where the owner's words are: 71 user decisions are in typed prompts and **37 are in `AskUserQuestion` outputs**, where the owner's answer to a question arrives as a tool output. 12 of the accepted assistant proposals are quoted in prompts, 11 in replies and 1 in an `AskUserQuestion` output.

## Zero-quota pass: a stub that answers no claims (`m3.py stub`)

The home is curated by a localhost OpenAI-compatible stub. It answers every curator request with no claims and every digest request with no lines, so nothing leaves the machine. This pass gives the windows and their tokens exactly, and it checks M2's coverage half on the dev transcripts.

Three homes were replayed from the same fixtures in the same order: their `(seq, ts, kind, session)` rows hash alike, so one label map serves all three. The binaries are main at 448085f (the first home) and the shrink branch (the other two).

| Home | Shrink | Windows | Estimated tokens sent | Largest window | Tool calls shown short | Coverage |
|---|---|---|---|---|---|---|
| first stub | no | 15,426 | 60.43M | 5,714 | - | 100% |
| `whole` | no | 15,426 | 60.43M | | 0 | 100% |
| `short` | yes | 3,493 | 17.64M | 5,690 | 38,024 | 100% |

- **M2's coverage half passes on the dev transcripts.** In each home the window ops run from seq 1 to the last record (71,063, the 70,904 events plus the rescan's tombstones), with no gap and no overlap. Every window is `curated`, or `covered` where it holds no text.
- The two homes without the shrink give the same windows and the same tokens, so the cut is deterministic.
- **The shrink sends 70.8% less**: 17.64M against 60.43M estimated tokens, in 3,493 windows instead of 15,426. That clears the −30% clause of spec 3.5 (a). The estimate above (−71%) was right.
- The token counts include each prompt's fixed part and its candidates. The stub answered no claims, so no session carried anything.
- Every record has a repository value (11 in all, over the 63 sessions). A session that ran outside a git checkout gets its folder as its repository, as the labels' `repo` field shows. All 63 dev pairs have both of their decisions in one repository, so repository scope never keeps a pair's later claim from seeing the earlier one as a candidate.

## What the curator is sent

The estimate below uses D7's coefficients (0.8 tokens per CJK character, 0.28 per other character) on the live home's records, with the window's rendering rules (a tool input cut at 2,000 characters; a tool output in full, or elided when it is over the window).

| Part | Estimated tokens | Share |
|---|---|---|
| Tool outputs | 39.7M | 77.9% |
| Tool inputs | 9.6M | 18.8% |
| Harness envelopes | 0.7M | 1.4% |
| Compaction summaries | 0.4M | 0.8% |
| Assistant replies | 0.4M | 0.7% |
| Typed prompts | 0.2M | 0.3% |
| Total | 51.0M | |

- 66,784 of the 70,904 records are tool calls. Bash alone accounts for 20.8M tokens of outputs and 4.9M of inputs, over 40,251 calls. The owner's own words and the assistant's replies, where the decisions are, are about 1% of what is sent.
- For a heavy day, at the owner's rate of about 4,000 events a day (docs/milestone-1.md, 362,379 events in 90 days), this is about 2.9M tokens and 580 windows a day. The free entries' daily caps (Groq about 200,000 tokens; 100 calls on the others in the baseline config) cannot take that, and the Cost line allows at most 20% of each cap. So at full size, curation would fall almost entirely on the subscriptions.

## Judge role (a): the deterministic shrink, declared before measuring

Spec 3.5 (a) asks for these conditions: deterministic rules come first; user and assistant messages are never filtered; the shrinking is recorded; and the role is enabled only if the curator's input shrinks by 30% or more while the recall of decisions, lessons and fixes drops by 0.02 or less.

The rule, with its values fixed now:
- A tool's input is shown up to **300** characters (today 2,000).
- A tool's output longer than **600** characters is shown as its first **300** and its last **300**, with a marker giving the characters left out. The tail keeps a test run's summary line.
- The outputs of the tools that carry the owner's own words are never shortened: `AskUserQuestion` and `ExitPlanMode` (Claude Code), and `request_user_input` and `request_user_input_async` (Codex). These tool names come from the data. 37 of the 132 matched labels are in `AskUserQuestion` outputs.
- Prompts, replies, compaction summaries and envelopes are not touched.
- The code gates still read the whole output: `passing_run` reads the line's source, and MUST-M4's taint reads the whole output of every tool line in the window. Only what is sent, and what the window's size counts, gets shorter.
- A window op lists the records it shortened, as it lists the ones it elided.

The estimate for this rule is 14.7M tokens, that is 71% less input (200/200/200 would give 77% less, and 500/500/500 would give 60% less). The stub pass with the rule in place will measure the real reduction.

## The live half: recall with and without the shrink

A full live run is not affordable: about 16,000 windows, 51M tokens. Recall is therefore measured on the labeled windows only:
- The base is a stub-covered home: every window is covered, with no claims.
- `oboete recurate <device>:<from>-<to> --yes` then sends the window of each labeled record to the real chain, one span at a time in seq order, so that an earlier claim is a candidate for a later window.
- This is done twice, once on a home cut with the shrink and once on a home cut without it.
- The home's config has `curate = false`, so no worker spends calls on digests afterwards.
- Before `--yes`, the estimate that `recurate` prints is recorded here as the run's budget.

Limits, the same for both arms:
- A session's carried context (its goal, proposals and open items) is empty, because the windows before a labeled one were covered by the stub. So absolute recall is lower than a full run would give, and only the difference between the arms is read.
- N is 45 owner-yes decisions, so a single label is 0.022, which is over the 0.02 line by itself. The raw counts are reported next to the ratios.
- One run per arm cannot separate one label's difference from the chain's own non-determinism. So a difference of one label is read as noise, and two or more count against the shrink.

## Definitions for scoring (fixed before any live number)

- A labeled decision is **recalled** when an active claim (the `active` view) has status `decided` and has an evidence quote in the record its label maps to.
- **Shown as current**: an active claim with status `decided` that no active derivation supersedes (no `edges` row from an active derivation to its uid).
- **Overturn pair** (the owner said "overturns"): the earlier decision's matched claims are counted as (a) still current, (b) superseded, (c) retracted, or (d) never derived. The 0% hard line applies to (a); (d) is reported as a recall miss.
- **Control pair** (the owner said "compatible"): the earlier decision must stay in (a). A control dropped counts against the 2% line.
- **Recall** counts the owner's yes (45). The panel's yes on the owner's 判断できない items (8) is reported apart, because the panel's κ on decisions was about 0 (docs/milestone-1.md).
- **Precision** is not computed from a panel, for the same reason. The run writes a sample of up to 100 `decided` claims for the owner (docs/milestone-3-plan.md, "What needs the owner", item 1). Only what is known is reported: the decided claims that map to an item the owner said no to, and the total number of decided claims.

## Found during the live half (2026-09-28)

- **Claude Code 2.1.283's built-in plugins** (`agents-md`, `telemetry`) made every claude curator call fail the isolation check. Fixed in #191 before the live half ran; the first call's failure is kept apart (`live-sent-failed-isolation.jsonl`).
- **haiku thinks at `--effort low`**: its calls return 9,000 to 16,000 completion tokens for 5,000 to 6,000 in, and take 80 to 130 s. A probe with `"alwaysThinkingEnabled": false` in `--settings` dropped the thinking block and took half the time on a small prompt. Whether the curator's recall holds without thinking is not measured; both arms here run with thinking, as shipped.
- **The owner's answers to `AskUserQuestion` are tool output to the gates.** The owner's answer is in the record's input and output, a JSON string of the questions and answers. The line's role is a tool's, so the speaker gate makes such a claim "tool result", and gate 1 then keeps it from `decided`. 37 of the 132 matched labels are such answers. In the first six live windows, 9 drafts were lowered for "the speaker is the quote's line" and 14 for "decided needs the user's words or an acceptance right after"; no claim reached `decided`. The fix, which is Task 8's code and touches MUST-M4 (a fake acceptance inside a tool output), is designed after this run's numbers.
