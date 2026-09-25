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
- A typed prompt is what the prompt hook received from the developer (`typed` in replay_set.py):
  - Claude Code: user records without the hook's envelopes, teammate messages, command output, bash mode, interrupts and local commands (`/compact`, `/effort`, ...: in either stored form). A skill command and `/goal` count, as `/name args`. A prompt typed while a turn runs counts when it is queued (`queue-operation` `enqueue`), and once only when it is later delivered.
  - Codex: user messages whose `content_item_kinds` say `user.text`; for older rollouts without it, messages that do not start with a harness prefix (`CODEX_CONTEXT`).
  - Checked against claude-mem's copy (its prompts are what the hooks received), on the sessions outside the held-out set: Codex 334 of 339 sessions give the same count; Claude Code 226 of 272 sessions give the same count, and 1,096 of claude-mem's 1,150 prompts match one of our 1,149 (95% each way). The rest are prompts merged or edited in the queue and small text differences.
- Sides: held-out = test side of the common session-hash split, dev = dev side.
- 24 Claude Code and 6 Codex sessions per side. Strata: length by tool calls, subagent files included (< 50, 50-299, ≥ 300) × language (Japanese when ≥ 30% of typed characters are Japanese). One per non-empty stratum first, the rest in proportion, never above the quota.
- Subagent files include workflow agents (`subagents/workflows/wf_*/agent-*.jsonl`): the live hook sees their tool calls (a session with 300 calls in its main file had 16,151 hook events).
- The longest-span transcript is added when it spans 20 hours or more and was not drawn already.

Pool on 2026-09-26: 227 Claude Code and 242 Codex sessions.

| Side | Agent | long/ja | long/en | mid/ja | mid/en | short/ja | short/en | Total |
|---|---|---|---|---|---|---|---|---|
| held-out | claude | 11 | 1 | 5 | 0 | 5 | 2 | 24 |
| held-out | codex | 1 | 1 | 1 | 1 | 1 | 1 | 6 |
| dev | claude | 11 | 2 | 5 | 1 | 3 | 2 | 24 |
| dev | codex | 1 | 1 | 1 | 1 | 1 | 1 | 6 |

- The long session: 58.97 hours, 22 typed prompts, 7,604 tool calls with its subagents, held-out (Claude Code), drawn in its stratum. No 24-hour session was found; this one is longer.
- Held-out: 123 typed prompts and 29,866 tool calls. Dev: 93 and 25,145. 1,844 files (with subagent files), 1.1 GB.
- Three draws were discarded before anything read them, and freeze.json's two replay entries removed each time:
  1. The first counted 1,063 task notifications as typed prompts, which made nearly every Claude Code session look English and short (issue #65).
  2. The second left subagent tool calls out of the length strata (PR #66 review).
  3. The third missed the workflow agents' files, 1,614 of them in 22 sessions, both in the strata and in the copies. It also counted local commands and Codex harness messages as typed, and missed queued prompts (PR B review, checked against claude-mem as above).

  The fourth draw, with the rules above, is the frozen one; it keeps 50 of the third draw's 61 sessions.

## Findings

- Today's hook keeps teammate messages ("Another Claude session sent a message: <teammate-message …>") as prompts: `ENVELOPES` in src/hook.rs has `<agent-message` but not this form. Design B's capture (milestone 2) should treat it as an envelope.
