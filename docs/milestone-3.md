# Milestone 3: Curate

The plan is docs/milestone-3-plan.md. The dev measurements behind this note, and their harness, are in docs/spike/m3-dev.md and docs/eval/m3.py. Every number here is from the dev split: no held-out transcript was read.

## Where it stands (2026-09-28)

- Tasks 1 to 11 are merged. The last of them are recuration (#189), the crash harness (#190), and claude's built-in plugins turned off for a curator call (#191, Claude Code 2.1.283).
- Task 12, the judge's first role ("shrink, never drop"), is behind `[summary] shrink` (#194), off by default. Its input clause passes on dev; its recall clause is measured again (below). Its second role (a veto on `decided` and `supersedes`) has no code: it waits on M3.
- Task 13 has the lines below: M2 passes, Cost fails with today's defaults, M3 fails on the dev labels. The first dev tuning of the gates is merged: the owner's answer to `AskUserQuestion` is the owner's words (#195, #202, #209).

## M2 on the dev transcripts: passes

- **Coverage.** The 63 dev sessions (70,904 events) were replayed in time order into one home and curated by a local stub. The window ops run from seq 1 to the last record with no gap and no overlap, and every window is `curated`, or `covered` where it holds no text. That holds with the shrink off (15,426 windows) and on (3,483 windows).
- **Crash.** A crash at 20 points of a curating run leaves the rows of a run with none (#190, `worker::tests::a_crash_at_twenty_points_gives_the_rows_of_a_run_with_none`).

## Isolation

The per-CLI table is in docs/spike/curator-isolation.md. Claude Code 2.1.283 added two built-in plugins (`agents-md`, `telemetry`) that load whatever the setting sources say. Every claude curator call failed the isolation check until #191 turned them off in `--settings`.

## Cost (per heavy day)

The line: curator calls take at most 20% of each daily cap (subscriptions have none, owner decision 30), and paid entries cost at most USD 5 a month.

A heavy day, measured: the owner's transcripts of the last 90 days (3,870 files under `~/.claude/projects` and `~/.codex/sessions`, counts only) hold 11,996 lines on the median day and 43,853 on the 90th-percentile day. At the 4.1 lines per event of milestone 1's count (362,379 events), the 90th-percentile day is about 10,700 events, 2.7 times the mean.

| On a heavy day | Windows | Estimated tokens |
|---|---|---|
| Shrink off | about 2,330 | about 9.1M |
| Shrink on | about 526 | about 2.65M |

(From the dev stub pass: 0.218 windows and 852 tokens per event with the shrink off, 0.049 and 248 with it on.)

- **The line fails with today's defaults.** Groq's free tier allows 1,000 requests and 200,000 tokens a day per model. The default chain gives each of its three Groq entries 800 calls a day and no token budget, so on a heavy day the curator takes all of each model's tokens, not 20%. Keeping to 20% means about 40,000 tokens a model, some 24 windows a day across the three. The rest of a heavy day falls to the subscriptions.
- **The subscriptions' speed is a limit too.** claude haiku takes about 100 s a window (212 calls: median 101 s, 99th percentile 187 s). The 526 windows of a heavy day with the shrink on are about 14.6 hours of calls in a row, and without the shrink about 65 hours. A day that heavy is curated over the following days, or by several entries at once.
- Paid entries: none in the default chain, so the USD 5 cap is not reached.
- With the curator's thinking off (measured below: 16.6 s a call at the median), the 526 windows are about 2.4 hours of calls in a row. That is a projection: no arm ran the shrink and thinking off together.
- What would meet the line is a decision about defaults: token budgets at 20% on the free entries, the shrink on once its recall clause passes, and the curator's thinking off (#193, now the default).

## M3 on the dev labels: fails

The line: recall of the owner's decisions 80% or more, no overturned decision shown as current (0%), and at most 2% of compatible earlier decisions dropped. Four arms were run, one live entry (claude haiku), one run each, over the windows that hold a label (docs/spike/m3-dev.md has the harness and the definitions, fixed before any live number).

| Arm | Binary | Shrink | Thinking | Recall (of 44) | Overturned, still current (of 19) | Compatible, dropped (of 25) | Owner-no records shown as decided (of 5) | Decided claims |
|---|---|---|---|---|---|---|---|---|
| whole | #194 + #191 | off | on | 10 | 2 | 0 | 4 | 65 |
| short | #194 + #191 | on | on | 13 | 3 | 0 | 1 | 57 |
| full | #195 (the owner's picks are the user's words) | off | on | 19 | 8 | 1 | 1 | 95 |
| nothink | #195 | off | off | 24 | 9 | 1 | 3 | 122 |

whole and short cut 114 spans, and some of them shared a record, so that record's window was sent twice and the second pass retracted the first pass's claims: 1 overlap and 23 claims retracted in whole, 5 overlaps and 202 retracted in short. Their rows are kept here, but they do not measure what they were meant to. full and nothink cut 109 spans, merged so that none shares a record (#196's commit), and retracted none. The table is `m3.py score`; the counts by kind, the drop classes and the records below were read after the fact with one-off queries over the same homes.

- **The owner's picks in `AskUserQuestion`** (#195): 11 of the 13 labels that are such picks reached `decided` in full, and none did in whole or short. That is the whole of the gain from whole to full (the typed prompts went from 10 to 8 of 28, run to run).
- **Recall by kind**, nothink: picks 10 of 13, typed prompts 14 of 28, accepted proposals quoted from the assistant's reply 0 of 3 (0 in every arm).
- **Instructions for the moment.** About 10 of the 44 labels are instructions whose effect ends with the session ("新セッションで実装する", "ブラウザ開かないからurl教えて", "codex-reviewが未実行ならそれだけ実行して"); this list is mine, not the owner's. Every arm recalled at most one of them. Without them, recall is 23 of 34 (68%) in nothink and 19 of 34 (56%) in full: still under 80%. Whether they count is the owner's call (below).
- **Overturns fail the 0% line** once recall rises: 8 and 9 of 19 in full and nothink. The curator does write `supersedes` (28 to 59 active edges per arm), but not for these pairs. 17 of the 19 pairs are in one session and 2 across sessions (d123 to d361, d490 to d187); several share one earlier record, so one missed supersede counts more than once (the per-record definition).
- **Why, read after the fact.** An earlier decision reaches a later window only as a candidate: up to 20 current claims of the repository that the full-text index finds for 64 trigrams spread over the window's text. What a session carries in is its goal, its previous window's proposals and its open items, not its decisions, so the harness's stub-covered windows do not hide it. Replaying that search for the 9 pairs left current in nothink (with the window's records standing in for its rendered text, and the index as it ended):
  - 54 to 63 of the 64 trigrams came from tool text in every later window, so the search mostly looks for words of tool output;
  - in 3 pairs (d151, d176, d142 as the earlier side) the earlier decision matched none of them and was not shown;
  - in 6 it was shown, near the bottom (rank 13 to 17, where 13 to 35 current claims matched), and the curator did not supersede it. With a repository's claims in the thousands rather than tens, a rank that low would not be shown.
- **The compatible pair dropped** in full and nothink is d121 to d361: the later decision ("このプロジェクトを破棄して…") superseded 案B. By the owner's labels 案B had already been replaced by d123, which d361 overturns. It counts as a drop by the definition; it is 1 of 25, 4%, over the 2% line.
- **Owner-no records**: the three in nothink (d71, d228, d410) are the three whole also showed. d71 is a long typed plan whose other sentences are rules; d228 and d410 are the assistant's reports of what it did, drafted as the user's decisions. Not a difference that thinking made.
- **What is not in the window**: drafts dropped because their quote is not in the window, nothink: 263 over 120 kept answers. 195 are paraphrases, 30 match once whitespace is removed, 15 quote a tool's input, 13 quote another record, 6 join two pieces with "...". A whitespace-blind match would keep the 30.

### Thinking (#193 item 3)

Measured once each on the #195 binary, after the fact: there was no rule declared before, as there was for the shrink.
- Recall 24 of 44 with thinking off, 19 with it on.
- A curator call took 16.6 s at the median with thinking off (90th percentile 28.7 s), 101 s with it on (150 s). 0 answers were refused with it off, 5 with it on (2 unanchored, 2 prose, 1 shape).
- Decided claims: 122 with it off, 95 with it on.

So thinking off is at least as good here and six times faster. It is the default since #219, a PR of its own so it can be reverted alone.

### Not measured together

The shrink was measured on the #194 binary with thinking on; thinking off on the #195 binary without the shrink. No arm ran the shrink, thinking off and #195 together, which is what the defaults would become.

### What the next M3 run needs

1. The owner's answer on instructions for the moment, and the test labels.
2. Supersedes: candidates found by the owner's and the assistant's lines rather than a spread over mostly tool text, and a prompt that asks for `supersedes` when the owner drops or redoes what an earlier decision set up.
3. The quote match: whitespace-blind, then the paraphrases.
4. One arm with the defaults as they would ship (shrink on, thinking off, the current main).

## Judge, role (a): the shrink: not established

- **Input**: 70.9% less (17.59M against 60.43M estimated tokens on the dev set), past the 30% the spec asks.
- **Recall**: the two arms that were to decide it (whole and short, above) sent overlapping spans, and short's second passes retracted 202 claims against whole's 23. So 13 against 10 does not measure the shrink. It is measured again on the current main, with merged spans, the shrink on and off, and the rule declared before (one label's difference is noise, two or more fewer fail).
- **Even if it passes**, it passes for decisions: the clause names lessons and fixes too, which have no dev labels. So the shrink stays off by default until they are measured as well (review on #220).

## Window

Not measured. The window is the smallest size that passes M2, M3 and M6 on the dev transcripts, and no size passes M3 yet: it is measured with the next M3 run.

## What needs the owner

1. Whether an instruction for the moment ("新セッションで実装する", "url教えて") counts as a decision the memory keeps. About 10 of the 44 dev labels are such instructions, and the curator keeps almost none; without them recall is 68% at best, with them 55%. It sets what M3 measures, and the test labels follow it.
2. M3's test labels (plan, "What needs the owner", item 1).
