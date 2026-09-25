# Milestone 1: freeze and label

The plan is docs/milestone-1-plan.md. The spec is docs/spec.md §8.1-8.4. Evaluation data lives only in ~/.oboete/eval (owner-only files); this note holds the rules, seeds and numbers.

## Frozen inputs

`python3 docs/eval/freeze.py check` must print `ok` before any later script runs. freeze.json lists each file's sha256.

| File | What | Frozen |
|---|---|---|
| queries.jsonl | the 424 questions: dev 312, test 112 (test: Japanese prompts 104, English prompts 6, English agent searches 1, Japanese agent searches 1) | Task 1 |
| claude-mem-2026-09-24.db | the copy the evaluation store and the 424 questions came from; newest session 2026-09-24T00:44:49Z | Task 1 |
| queries-en.jsonl | M21's 53 new English test questions from 51 test-side sessions, unjudged until milestone 4 (Appendix C item 13, A86) | Task 2 |

The evaluation store (~/.oboete/eval/home/oboete.db) is rebuilt from the copy, so it is recorded by counts (2026-09-26): 152,136 observations, 13,169 summaries, 13,197 prompts, highest observation id 152,136. That is 178,502 documents, the store D2 was measured on (docs/pr-d.md:14). docs/pr-b.md's B1 counts (152,030 / 13,155 / 13,185) describe an earlier state of the same store.

## Seeds

Every draw orders candidates by sha256 of `<purpose>:oboete-milestone-1-2026-09-26:<id>` (docs/eval/common.py SEED).

## The unseen final set (spec 8.1), sealed now, drawn at milestone 8

- Source: a fresh read-only copy of claude-mem's database taken at milestone 8, imported into a new evaluation home; questions built with build_queries.py's filters.
- Sessions: only those whose `sdk_sessions.started_at` is after 2026-09-24T00:44:49Z. Excluded: every session behind queries.jsonl, queries-en.jsonl and replay/manifest.json, and any session whose results were seen before milestone 8.
- Size and strata: at least 112 questions, in the test split's proportions: Japanese prompts 104, English prompts 6, English agent searches 1, Japanese agent searches 1, scaled to the drawn size.
- Order: sha256 of `final:oboete-final-2026-09-26:<qid>` within each stratum.
- End-to-end: at least 10 new held-out transcripts from the same period, drawn with replay_set.py's strata and the seed `oboete-final-replay-2026-09-26`.
- Judged only at milestone 8, by a judge that passed calibration. Nobody tunes on it. A failed final run uses the set up (8.1).
- Risk: if claude-mem stops recording before milestone 8, the source becomes the agents' transcripts from the same period, with the same filters; the decision is recorded here when it happens.

## Replay set (Task 3)

Frozen as replay/manifest.json (every copied file's sha256) and replay/events-1000.jsonl. Rules (Claude; overrulable):
- Pool: transcripts on disk whose session claude-mem recorded with at least one observation (so claude-mem's own rows are its end-to-end baseline, for Claude Code and Codex alike), with at least one typed prompt.
- Typed prompts leave out Claude Code records the developer did not type: the hook's envelopes, teammate messages, command output, slash-command tags, bash mode and interrupts (`NOT_TYPED` in replay_set.py).
- Sides: held-out = test side of the common session-hash split, dev = dev side.
- 24 Claude Code and 6 Codex sessions per side. Strata: length by tool calls (< 50, 50-299, ≥ 300) × language (Japanese when ≥ 30% of typed characters are Japanese). One per non-empty stratum first, the rest in proportion, never above the quota.
- The longest-span transcript is added when it spans 20 hours or more.

Pool on 2026-09-26: 224 Claude Code and 242 Codex sessions.

| Side | Agent | long/ja | long/en | mid/ja | mid/en | short/ja | short/en | Total |
|---|---|---|---|---|---|---|---|---|
| held-out | claude | 8 | 2 | 7 | 1 | 5 | 2 | 25 (with the long session) |
| held-out | codex | 1 | 1 | 1 | 1 | 1 | 1 | 6 |
| dev | claude | 7 | 1 | 8 | 2 | 4 | 2 | 24 |
| dev | codex | 1 | 1 | 1 | 1 | 1 | 1 | 6 |

- The long session: 58.97 hours, 18 typed prompts, 3,864 tool calls, held-out (Claude Code). No 24-hour session was found; this one is longer.
- Held-out: 145 typed prompts and 15,934 tool calls. Dev: 125 and 12,175. 304 files (with subagent files), 706 MB.
- The first draw was discarded before anything read it: its "typed prompts" counted 1,063 task notifications, which made nearly every Claude Code session look English and short. freeze.json's two replay entries were removed and the set drawn again with the rules above (issue #65).

## Findings

- Today's hook keeps teammate messages ("Another Claude session sent a message: <teammate-message …>") as prompts: `ENVELOPES` in src/hook.rs has `<agent-message` but not this form. Design B's capture (milestone 2) should treat it as an envelope.
