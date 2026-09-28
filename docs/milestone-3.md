# Milestone 3: Curate

The plan is docs/milestone-3-plan.md. The dev measurements behind this note, and their harness, are in docs/spike/m3-dev.md and docs/eval/m3.py. Every number here is from the dev split: no held-out transcript was read.

## Where it stands (2026-09-28)

- Tasks 1 to 11 are merged. The last of them are recuration (#189), the crash harness (#190), and claude's built-in plugins turned off for a curator call (#191, Claude Code 2.1.283).
- Task 12, the judge's first role ("shrink, never drop"), is behind `[summary] shrink`, off by default (#194). Its second role (a veto on `decided` and `supersedes`) has no code: it waits on M3's dev recall.
- Task 13 has the lines below. The first dev tuning of the gates is merged: the owner's answer to `AskUserQuestion` is the owner's words (#195, #202).

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
- What would meet the line is a decision about defaults, taken with M3's dev result: token budgets at 20% on the free entries, the shrink on, and (#193) the curator's thinking.

## M3 on the dev labels

(Filled in when the live arms end: recall per arm, the causes, the owner's question.)

## Judge, role (a): the shrink

- Input: 70.9% less (17.59M against 60.43M estimated tokens on the dev set), past the 30% the spec asks.
- Recall: (filled in when the live arms end).

## Window

Not measured yet. The window is the smallest size that passes M2, M3 and M6 on the dev transcripts. It waits for M3's dev recall to leave the floor.

## What needs the owner

1. Whether a one-off instruction ("新セッションで実装する", "url教えて") counts as a decision the memory keeps. It sets what M3 measures, and the test labels follow it.
2. M3's test labels (plan, "What needs the owner", item 1).
