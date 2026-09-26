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

## Transcript parser (Task 4)

`oboete transcript <file> --agent claude|codex` (hidden) prints a transcript as the replay fixture `oboete replay` reads: the hook events the transcript implies, as the live hooks received them. Run on the 30 dev transcripts of the fourth draw (2026-09-26, read-only):

| Agent | Files | Lines | Unreadable | Events | Tool events | Prompts: typed / envelopes |
|---|---|---|---|---|---|---|
| Claude Code (with 968 subagent files) | 24 | 129,180 | 0 | 22,169 | 20,921 | 73 / 533 |
| Codex | 6 | 29,494 | 0 | 4,271 | 4,224 | 20 / 0 |

- The parser and replay_set.py are two implementations of the same rules (Task 3); they agree on every session's typed prompts and tool calls.
- Envelopes (task notifications and the like) are emitted as prompts because the live hook receives them: the live store had 39 in September. The hook's `ENVELOPES` decides what to keep.
- Slash commands: a skill command (stored message tag first) and `/goal` are prompts, as typed (`/name args`); claude-mem's copy has `/goal` prompts from 42 sessions. The other local commands (`/compact`, `/effort`, ...; stored name tag first, or as plain text by older Claude Code; 713 of them in 1,909 transcripts outside the held-out set) are not: claude-mem's 15,218 prompts have none of them.
- A prompt typed while a turn runs is sent when it is queued, and its later delivery is skipped. The running turn goes on: in the dev set, 20 such prompts were delivered later as user records, none between a turn's tool calls (14 right after the enqueue, 4 right after a turn's end record, 2 after other records), and 329 queued messages (typed or not) reached a running turn as attachments.
- Failed tool calls carry `error` and no `tool_response`, as live failures do (the plan's table asked for both; the hook reads `tool_response` first, so replay would have stored a different text).
- Workflow agents' files (`subagents/workflows/wf_*/agent-*.jsonl`) are read with the other subagent files; a workflow's `journal.jsonl` is not a transcript and is skipped.
- Codex: harness messages are left out by `content_item_kinds`; a forked rollout keeps its own id (its parent's `session_meta` follows its own); a resumed rollout follows `turn_context` into its new directory; an aborted turn sends no Stop.
- A subagent's compacted context is not the session's PostCompact.
- Lines come out in time order (a stable sort), so subagent calls sit inside the turn that made them; Stop comes where the Stop hook ran (`stop_hook_summary`) or the turn ended (`turn_duration`), else at the next prompt, with the directory of the turn's last text; calls a developer interrupted (Claude Code) or a turn abort cut off (Codex) end there. The largest dev session (111 MB with its subagent files) converts in 0.3 s with a 39 MB peak.
- Record types not read. Claude Code: agent-name, ai-title, atis-latch, attachment, bridge-session, cost-state, custom-title, file-history, fork-context-ref, frame-link, last-prompt, mode, permission-mode, pr-link, queue operations other than enqueue, system records other than the two turn ends. Codex: reasoning, token counts, turn and thread bookkeeping, inter-agent messages, world_state. None is typed dialogue.

## B3: judge trust (Task 6, 2026-09-26)

By a panel, not the owner (owner decision 29: the owner found the memories too English and technical to judge). The 50 pairs of `calib.py draw` (dev questions only; 25 graded relevant by the judge, 12 graded 1, 13 graded 0; one per question) were graded by five API judges from other makers, each pair alone, with judge.py's prompt and the text the judge saw (`labels/calib-50.inputs.jsonl`), temperature 0. The judge under test counts with its pool grades (`claude-sonnet-5`, graded in batches of 10). Relevant = grade ≥ 2.

B3 is decided on run 2. Run 1 took the first entry of any answer; Codex (#77) asked for exactly the grade of the one memory asked about, and run 2, asked again under that rule, found 3 answers in run 1's style (grades for "1", "2", "3"), so run 1 may hold misread grades. Run 1 stays frozen as it was.

| Judge (maker, model) | Run 2: κ against the other five's majority (49 pairs) | Agreement | Run 1 κ |
|---|---|---|---|
| Anthropic `claude-sonnet-5` (under test) | 0.88 | 94% | 0.84 |
| OpenAI `openai/gpt-oss-120b` (Groq) | 0.68 | 84% | 0.60 |
| DeepSeek `deepseek-v4-pro` (OpenCode Go) | 0.88 | 94% | 0.72 |
| Zhipu `glm-5.3` (OpenCode Go) | 0.75 | 88% | 0.72 |
| Moonshot `kimi-k3` (OpenCode Go) | 0.96 | 98% | 0.76 |
| Alibaba `qwen3.8-max` (OpenCode Go) | 0.80 | 90% | 0.76 |

- The panel's Fleiss κ: 0.74 (run 1: 0.72).
- **B3 passes** (each judge κ ≥ 0.4, panel ≥ 0.4, in both runs). The judge may decide from here on; D2's 0.545 is no longer provisional on this account.
- Three answers stayed unusable after three tries, all on pair c29 (gpt-oss-120b, kimi-k3, glm-5.3 graded memories "1", "2", "3"). c29 is left out for every judge, like a tie (spec 8.1), so all six are measured on the same 49 pairs, each against all five others (`labels/calib-50.result-2b.json`). The first result of run 2, which left c29 out for those three judges only, is frozen as `labels/calib-50.result-2.json`; its numbers differ by less than 0.005.
- The model each provider reports is recorded with each grade from now on; run 2's replies did not record it, so its models are the requested ones.
- Stability: between the two runs, at temperature 0 and on the same inputs, 41 of 247 grades changed, 18 of them across the relevant line (7%). Hosted models are not deterministic at temperature 0; a single run's κ carries that noise.
- What this does not show (spec 8.1): judges that share a bias agree on the same wrong grade. Agreement with the owner is measured later, on the owner's own decisions (#76).
- Frozen: `labels/calib-50.inputs.jsonl` (the question and memory every panel judge read, from the evaluation store, unchanged since 2026-09-24), run 1 (`labels/calib-50.panel.jsonl`, `labels/calib-50.result.json`) and run 2 (`labels/calib-50.panel-2.jsonl`, `labels/calib-50.result-2.json`, `labels/calib-50.result-2b.json`). The 4 answers the owner gave before decision 29 are set aside, unused (`labels/calib-50.withdrawn-2026-09-26.jsonl`).

## M22 corpus (Task 10)

`docs/eval/corpus_count.py`, 2026-09-26, read-only (~/.oboete/eval/corpus-count.json):

| Store | Observations | Summaries | Prompts | Other |
|---|---|---|---|---|
| Eval store (claude-mem copy imported, 2026-09-24) | 152,136 | 13,169 | 13,197 | |
| Live store (today's oboete on WSL) | 169 | 20 | 23 | 17,939 events, all in the last 7 days |
| claude-mem on Windows | 1,285 | 216 | 195 | |
| claude-mem on the iMac | none | | | no database |

- Documents a device would hold if everything were imported: 178,502 (the eval store) + 1,696 (Windows' claude-mem) = about 180,200. This is an upper bound: the machines' histories overlap, and the live store's 212 documents cover sessions claude-mem also recorded.
- That is about half the old "~330k" figure, which counted claude-mem twice (spec 8.2).
- Raw events, from the transcripts of the last 90 days (3,319 files, parsed by `oboete transcript`): 362,379 events, so about 1.47 million a year. The fixture JSON is 2.14 GB for the 90 days, about 8.7 GB a year. That is uncompressed JSON, an upper bound before per-record compression, not a disk estimate.

## Dev label drafts (Task 8, 2026-09-26)

Drafted by `claude-sonnet-5` in owner decision 29's form: one plain-Japanese sentence and the owner's own message (a typed prompt, or their answer to a question). Every line gated, every quote verbatim.
- From the replay set's 30 dev transcripts: 140 decisions drafted, 99 kept from 21 sessions; 23 pairs, only 6 of them overturns. Line ids had to be normalized first (the model writes `4`, `"4"` and `"[L4]"` as well as `"L4"`; 67 drafts had been dropped for that alone).
- So 40 more dev-split transcripts were added (`draft_candidates.py extra 40`: claude-mem-recorded sessions outside the replay set, most typed prompts first, never the held-out side), copied owner-only under `replay/dev-extra`.
- Now: 428 decisions from 60 sessions; 181 pairs (45 overturns, 136 compatible). The first count said 204: a repository with more than 80 decisions is drafted in overlapping prompts, and 56 pairs came back more than once (7 of them with both relations); the first answer about a pair now stands. Then an accepted proposal took the time of its acceptance, not of the proposal (79 accepted, 26 of them moved): 2 earlier pairs no longer held their order and were dropped, and the prompts whose order changed found 15 more (Codex, #78). Last, every decision took the time of the developer's own message: 42 had been cited from a later restatement, and 5 compatible pairs no longer held their order and were dropped; no pair prompt changed, and none of the 43 owner pair tasks was affected (Codex, #78). Then a message in the 10-line overlap of two windows had its decisions drafted twice, with different quotes: a later window now adds decisions on a message only when the earlier one found none there. 92 of 520 went (ids and content of the rest unchanged); 34 pairs went with them, and the 15 pair prompts whose decisions changed found 59 more (Codex, #78).
- Pairs are grouped by the session's first working directory. A session that moved into another repository, or two worktrees of one repository, are grouped apart; that can only leave some pairs unproposed, since the owner gives each shown pair its relation.
- Read by Claude before handing over: 5 decisions, all real decisions in plain words; Latin letters only in product names (11 of the first 99). 5 overturn drafts: all real decisions, but not all overturns (2-3 of 5 are clear), and several stay technical in plain Japanese (version control, secret tags); the owner answers 判断できない there and the panel takes them.
- Tasks for the owner: 50 decisions (`dev-decisions`) and 40 pairs (`dev-pairs`: 20 drafted as each relation, cross-session first). Drawn again after the overlap fix, before any answer; the earlier tasks are kept owner-only under `labels/drafts/superseded-2026-09-26`. `draft_candidates.py tasks` adds more as answers of 判断できない come in.

## Dev labels (Task 8, 2026-09-26)

The owner answered every task shown: 59 decisions and 63 pairs; the last answer was on 2026-09-26 at 17:18 JST. The panel is `calib.PANEL`: seven judges from six makers, each of which passed B3 in calib-50 run 3. Every judge answered with one model in this run (`one_model_per_judge`). For grok-4.7 and gpt-6-astra this is the model their calibration recorded (run 3). The five API judges were calibrated in run 2, whose replies kept no model, so their continuity since then is not verified; `moved_since_calibration` cannot see them. What limits the risk: each was asked for a concrete model id, not an alias like "latest", and each reported exactly that id in this run (`openai/gpt-oss-120b`, `deepseek-v4-pro`, `glm-5.3`, `kimi-k3`, `qwen3.8-max`). A recalibration of the five with the model recorded would close this; it waits for Groq's daily budget, like the pair gap below. The panel graded the items the owner could not judge, plus a seeded sample of 40 the owner did judge, blind to the owner's answers (#76). Numbers are from `draft_candidates.py report` (`labels/dev-labels.result.json`, owner-only).

| | Owner | Panel on the owner's 判断できない | Panel majority vs owner on the sample |
|---|---|---|---|
| Decisions | yes 45, no 5, 判断できない 9 (target 50 counted: met) | 9 of 9 graded: yes 8, no 1 | n 40, agreement 0.875, κ −0.04 |
| Pairs | overturns 20, compatible 26, 判断できない 17 (target 20 overturns with 20 controls: met) | 15 of 17 with all seven votes: overturns 9, compatible 6 | n 6 so far (see below) |

- **Decisions: the panel says yes to almost everything.** On the 40-item sample the owner said yes to 36 and the panel majority to 39. The panel said yes to all 4 items the owner rejected. So agreement is high (0.875), and κ is about zero: with 36 of 40 yes, agreeing is what chance alone gives. For each judge, κ runs from −0.11 to 0.29 and agreement from 0.70 to 0.90. The panel's yes on the owner's 判断できない items (8 of 9) is therefore weak evidence that a decision was really made. Treat these as decision candidates, not as confirmed decisions. This is agreement on the owner's own decisions, not a check of technical relevance.
- **Pairs are not complete.** Groq's free tier allows gpt-oss-120b 200,000 tokens a day, on a rolling window. The calibration and panel runs of 2026-09-26 used it up: "Used 199724, Requested 1435" on the next call. So gpt-oss-120b graded 21 of the 57 pair targets; the other six judges graded all 57. A majority counts only with all seven votes, so 36 pair targets are listed as `panel_incomplete` and left out, never decided by six. The panel's pair numbers above cover what is complete. They are filled in when gpt-oss-120b's daily budget allows, and the pair rows are frozen then.
- **Frozen now** (`freeze.py add`, 22 files in all): the owner's answers, keys and tasks for both sets; the drafts the panel prompts are built from (`labels/drafts/decisions.jsonl`, `labels/drafts/pairs.jsonl`); and the decision panel rows (`labels/dev-decisions.panel.jsonl`). `labels/dev-pairs.panel.jsonl` and `labels/dev-labels.result.json` wait for the pair gap to close.
- **Blind repeat:** it opens a week after the last answer, on 2026-10-03 (`draft_candidates.py repeat`, then `agreement`).

## Fixtures, today's code (Task 9 step 5, 2026-09-26)

Task 7's fixtures through the owner's binary built from main f43da4d, in a temporary home with the API-only chain (`baseline.py config`):
- `middle-only`: 12 observations; the middle decision is captured ("同期間隔を45秒に設定"). The plan's query (`LIKE '%45 秒%'`, with a space) found 0 because the title has no space; `LIKE '%45%'` finds it in 2 observations. So #57's parts-in-order stopgap holds on this fixture.
- `long-24h`: 3 observations about the cache ("キャッシュの保存先をSQLiteに変更", "キャッシュ層の削除を決定", "キャッシュ保持に対する開発者の方針変更"), the overturned decision and the current one side by side with no status. As spec 8.1 expects, the M3 test milestone 3 writes fails against today's code.
- Steps 3 and 4 (the dev replay through today's oboete and the judged retrieval baselines) run outside the owner's working hours: they share the owner's free tiers and Claude allowance.

## Findings

- Today's hook keeps teammate messages ("Another Claude session sent a message: <teammate-message …>") as prompts: `ENVELOPES` in src/hook.rs has `<agent-message` but not this form. Design B's capture (milestone 2) should treat it as an envelope.
