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
- **Run 4 (2026-10-02): a new OpenAI judge.** Groq's free tier, which served `gpt-oss-120b`, is spent each day by the owner's own curation, so the owner asked to use the codex subscription instead (milestone 4 needed the panel then; docs/milestone-4.md). `gpt-6-sol` took `gpt-oss-120b`'s place, graded the same 50 pairs as the dogfood user (`calib.cli_chat`), and passed: κ 0.84 against the other seven judges' majority on 49 pairs (c29 still left out), agreement 0.92; the panel's Fleiss κ is 0.78 and every judge passes (`labels/calib-50.result-4.json`). `gpt-6-luna`, graded first, passed too (κ 0.67, `labels/calib-50.result-4-luna.json`) before the owner allowed a stronger model; `gpt-6.1-sol` answered 400 to a ChatGPT account on codex-cli 0.155.1. Frozen: `labels/calib-50.panel-4.jsonl` (both judges' rows; only the panel's are read) and the two results. A Workers AI route for `gpt-oss-120b` tried before it was stopped after 8 pairs, 3 of them unusable (`labels/calib-50.panel-4-workers-ai-stopped.jsonl`, unused).

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
| Pairs | overturns 20, compatible 26, 判断できない 17 (target 20 overturns with 20 controls: met) | 16 of 17 with all seven votes: overturns 10, compatible 6 | n 40, agreement 0.875, κ 0.75 |

- **Decisions: the panel says yes to almost everything.** On the 40-item sample the owner said yes to 36 and the panel majority to 39. The panel said yes to all 4 items the owner rejected. So agreement is high (0.875), and κ is about zero: with 36 of 40 yes, agreeing is what chance alone gives. For each judge, κ runs from −0.11 to 0.29 and agreement from 0.70 to 0.90. The panel's yes on the owner's 判断できない items (8 of 9) is therefore weak evidence that a decision was really made. Treat these as decision candidates, not as confirmed decisions. This is agreement on the owner's own decisions, not a check of technical relevance.
- **Pairs: the panel agrees with the owner beyond chance.** On the 40-pair sample the majority agrees with the owner on 35 (0.875, κ 0.75); per judge, κ runs from 0.55 (gpt-oss-120b) to 0.75 and agreement from 0.775 to 0.875. Unlike the decisions, the owner's answers here are split (overturns and compatible), so κ is informative. The panel's grades on the owner's 判断できない pairs are usable as labels with that caveat.
- **One pair stays ungraded.** Groq's free tier allows gpt-oss-120b 200,000 tokens a day on a rolling window, so its pair grades were filled in over 2026-09-26 to 2026-09-28 (a loop retrying every 30 minutes, `panel_slow.sh`): 56 of the 57 pair targets. The 57th, `d74-d78`, is over the free tier's per-request limit (8,000 tokens, HTTP 413), so it can never be graded there. A majority counts only with all seven votes, so it stays in `panel_incomplete`, left out and never decided by six. Grading it would need Groq's paid Developer tier (a card on the owner's account), which is not worth it for one pair (Claude; overrulable).
- **Frozen** (`freeze.py add`, 24 files in all): the owner's answers, keys and tasks for both sets; the drafts the panel prompts are built from (`labels/drafts/decisions.jsonl`, `labels/drafts/pairs.jsonl`); the decision panel rows (`labels/dev-decisions.panel.jsonl`); and, once the pair grades were in, the pair panel rows (`labels/dev-pairs.panel.jsonl`) and the result (`labels/dev-labels.result.json`), on 2026-09-27 at 19:35 UTC. `freeze.py check` passes.
- **Blind repeat:** it opens a week after the last answer, on 2026-10-03 (`draft_candidates.py repeat`, then `agreement`).

## Fixtures, today's code (Task 9 step 5, 2026-09-26)

Task 7's fixtures through the owner's binary built from main f43da4d, in a temporary home with the API-only chain (`baseline.py config`):
- `middle-only`: 12 observations; the middle decision is captured ("同期間隔を45秒に設定"). The plan's query (`LIKE '%45 秒%'`, with a space) found 0 because the title has no space; `LIKE '%45%'` finds it in 2 observations. So #57's parts-in-order stopgap holds on this fixture.
- `long-24h`: 3 observations about the cache ("キャッシュの保存先をSQLiteに変更", "キャッシュ層の削除を決定", "キャッシュ保持に対する開発者の方針変更"), the overturned decision and the current one side by side with no status. As spec 8.1 expects, the M3 test milestone 3 writes fails against today's code.
- Steps 3 and 4 (the dev replay through today's oboete and the judged retrieval baselines) run outside the owner's working hours: they share the owner's free tiers and Claude allowance. Both ran on the night of 2026-09-27 (below).

## Dev baselines (Task 9 steps 3 and 4, 2026-09-27)

### Curation: today's oboete and claude-mem on the 30 dev sessions

`python3 baseline.py oboete` replayed the 30 dev sessions through the owner's binary of that night (executable sha256 `5fa472f04ab6`, installed before #93) with the API-only chain of `baseline.py config` (groq, groq-20b, nim, opencode-go, openrouter, mistral; 100 calls a day each). It ran from about 01:20 to 04:10 JST, and the three sessions the budgets stopped were finished from 09:26 to 10:53 JST (below). `baseline.py claude-mem` took claude-mem's own rows for the same sessions from the frozen copy: 30 sessions, 4,611 observations. Counts only; no text was printed.

| session | agent | stratum | oboete observations | oboete summaries | claude-mem observations | claude-mem summaries |
|---|---|---|---|---|---|---|
| fdd4f4db | claude | claude/long/en | 12 | 1 | 9 | 0 |
| ef5ceb1d | claude | claude/long/en | 20 | 2 | 28 | 3 |
| 57d1fd58 | claude | claude/long/ja | 67 | 6 | 429 | 23 |
| a2fec979 | claude | claude/long/ja | 22 | 2 | 223 | 12 |
| f7dd2c15 | claude | claude/long/ja | 57 | 6 | 199 | 11 |
| 4b4ff68d | claude | claude/long/ja | 36 | 4 | 192 | 26 |
| cccab57b | claude | claude/long/ja | 30 | 3 | 30 | 1 |
| b61a5b7e | claude | claude/long/ja | 42 | 4 | 233 | 18 |
| 99de77cf | claude | claude/long/ja | 9 | 1 | 29 | 2 |
| 6e1c07b0 | claude | claude/long/ja | 22 | 2 | 78 | 32 |
| 8c03f2eb | claude | claude/long/ja | 42 | 5 | 244 | 5 |
| 687b6a27 | claude | claude/long/ja | 12 | 2 | 29 | 15 |
| 44ceb1b9 | claude | claude/long/ja | 276 | 29 | 417 | 93 |
| 256eb7ae | claude | claude/mid/en | 20 | 2 | 64 | 4 |
| d844d3b2 | claude | claude/mid/ja | 18 | 2 | 86 | 16 |
| 8c5e20c4 | claude | claude/mid/ja | 12 | 1 | 85 | 1 |
| 2e2d0bfd | claude | claude/mid/ja | 11 | 1 | 8 | 1 |
| a8b4c575 | claude | claude/mid/ja | 12 | 1 | 3 | 1 |
| 02816666 | claude | claude/mid/ja | 19 | 2 | 128 | 25 |
| d6861698 | claude | claude/short/en | 8 | 1 | 4 | 0 |
| 29b244be | claude | claude/short/en | 6 | 1 | 4 | 1 |
| 6218b171 | claude | claude/short/ja | 12 | 1 | 35 | 2 |
| deef39e9 | claude | claude/short/ja | 11 | 1 | 1 | 0 |
| 568a3bac | claude | claude/short/ja | 2 | 1 | 6 | 0 |
| 01a06da9 | codex | codex/long/en | 10 | 1 | 3 | 0 |
| 01a04ea8 | codex | codex/long/ja | 11 | 1 | 1990 | 1 |
| 01a07645 | codex | codex/mid/en | 6 | 1 | 21 | 0 |
| 01a036ff | codex | codex/mid/ja | 59 | 6 | 31 | 2 |
| 01a08bb9 | codex | codex/short/en | 8 | 1 | 1 | 1 |
| 01a09b95 | codex | codex/short/ja | 6 | 1 | 1 | 1 |

| provider | outcome | calls |
|---|---|---|
| groq | budget | 18 |
| groq | error | 130 |
| groq | ok | 6 |
| groq | wait | 4 |
| groq-20b | budget | 32 |
| groq-20b | error | 122 |
| groq-20b | ok | 11 |
| groq-20b | wait | 6 |
| mistral | error | 51 |
| nim | budget | 22 |
| nim | error | 33 |
| nim | invalid | 99 |
| nim | ok | 2 |
| opencode-go | error | 44 |
| opencode-go | ok | 23 |
| openrouter | error | 41 |
| openrouter | invalid | 5 |
| openrouter | ok | 50 |

- **All 30 sessions are finished.** The first night's daily budgets ran out (`budget` rows for groq, groq-20b and nim) with `44ceb1b9` (1,557 events), `01a036ff` (30) and `01a09b95` (35) partly unsummarized. They were finished on 2026-09-27 with the same binary, rebuilt from `v1` 431bafc (executable sha256 `5fa472f04ab6`, checked before the run), by repeating `observe --settle-ms 0` on the same home until no event was left. The owner's own binary had been reinstalled from `v1` 9799713 that morning, so it was not used. The provider table counts the calls of both runs.
- The provider table is the failure pattern that docs/research/curator-providers-2026-09-27.md analyzes, on the dev sessions: nim answered 2 of 134 calls (99 `invalid`, the truncation #93 fixed), groq 6 of 136, mistral 0 of 51. openrouter (50) and opencode-go (23) carried most windows.
- oboete keeps fewer records than claude-mem: 878 observations against 4,611. `01a04ea8` alone has 1,990 claude-mem observations. Whether fewer is worse is for the milestone 3 scorers; this task only stores the outputs they will score.

### Retrieval on dev

judge: claude-sonnet-5, trusted by B3 (κ = 0.88, run 2)

`judge.py dev 312 600` graded every pooled pair (171 new ones came with `hybrid-d2`'s dev rows, copied from `runs-test/` as step 4 says), then `uv run --with ranx python report.py dev`. 306 of the 312 dev questions have at least one relevant document; the other 6 are left out of the scores. M1's four systems are `e0-trigram` (today's FTS), `hybrid-d2` (today's hybrid), `claude-mem` and `claude-mem-nowindow`; the other rows are the earlier runs already in `~/.oboete/eval/runs`. A superscript letter means the row is better than that row at p < 0.05 (ranx `compare`, paired). `-l2`: only grades 2 and 3 count as relevant.

```
split=dev judge=claude-sonnet-5 questions=312 judged=312 with an answer=306 without=6

## all: 306 questions
#    Model                NDCG@10     MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  ----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.282ᵇᵈ     0.337ᵇᵈ      0.073ᵇᵈ         0.578ᵇᵈ
b    claude-mem           0.219ᵈ      0.230ᵈ       0.048ᵈ          0.392ᵈ
c    e0-trigram           0.436ᵃᵇᵈ    0.565ᵃᵇᵈ     0.149ᵃᵇᵈ        0.817ᵃᵇᵈ
d    fts                  0.048       0.094        0.012           0.098
e    hybrid-d2            0.583ᵃᵇᶜᵈ   0.706ᵃᵇᶜᵈ    0.238ᵃᵇᶜᵈ       0.918ᵃᵇᶜᵈ
f    hybrid-kf            0.586ᵃᵇᶜᵈ   0.731ᵃᵇᶜᵈᵉʰ  0.235ᵃᵇᶜᵈ       0.922ᵃᵇᶜᵈ
g    hybrid-rrf           0.591ᵃᵇᶜᵈʰ  0.749ᵃᵇᶜᵈᵉʰ  0.236ᵃᵇᶜᵈ       0.922ᵃᵇᶜᵈ
h    vec-bge-m3           0.564ᵃᵇᶜᵈ   0.668ᵃᵇᶜᵈ    0.223ᵃᵇᶜᵈ       0.902ᵃᵇᶜᵈ
i    vec-kf               0.592ᵃᵇᶜᵈʰ  0.732ᵃᵇᶜᵈʰ   0.245ᵃᵇᶜᵈ       0.918ᵃᵇᶜᵈ

## question in Japanese: 266 questions
#    Model                NDCG@10     MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  ----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.251ᵇᵈ     0.303ᵇᵈ      0.059ᵇᵈ         0.538ᵇᵈ
b    claude-mem           0.190ᵈ      0.198ᵈ       0.034ᵈ          0.342ᵈ
c    e0-trigram           0.419ᵃᵇᵈ    0.545ᵃᵇᵈ     0.141ᵃᵇᵈ        0.805ᵃᵇᵈ
d    fts                  0.016       0.038        0.003           0.038
e    hybrid-d2            0.570ᵃᵇᶜᵈ   0.688ᵃᵇᶜᵈ    0.230ᵃᵇᶜᵈ       0.910ᵃᵇᶜᵈ
f    hybrid-kf            0.577ᵃᵇᶜᵈ   0.721ᵃᵇᶜᵈᵉ   0.232ᵃᵇᶜᵈ       0.921ᵃᵇᶜᵈ
g    hybrid-rrf           0.578ᵃᵇᶜᵈʰ  0.728ᵃᵇᶜᵈᵉʰ  0.231ᵃᵇᶜᵈ       0.914ᵃᵇᶜᵈ
h    vec-bge-m3           0.553ᵃᵇᶜᵈ   0.661ᵃᵇᶜᵈ    0.226ᵃᵇᶜᵈ       0.910ᵃᵇᶜᵈ
i    vec-kf               0.585ᵃᵇᶜᵈʰ  0.714ᵃᵇᶜᵈ    0.240ᵃᵇᶜᵈ       0.914ᵃᵇᶜᵈ

## question in English: 40 questions
#    Model                NDCG@10     MRR@10-l2     Recall@10-l2    Hit Rate@10-l2
---  -------------------  ----------  ------------  --------------  ----------------
a    claude-mem-nowindow  0.487ᵇᵈ     0.564ᵇ        0.171ᵇᵈ         0.850ᵇᵈ
b    claude-mem           0.414ᵈ      0.446         0.142ᵈ          0.725ᵈ
c    e0-trigram           0.549ᵇᵈ     0.703ᵇᵈ       0.206ᵇᵈ         0.900ᵇᵈ
d    fts                  0.259       0.471         0.070           0.500
e    hybrid-d2            0.667ᵃᵇᶜᵈ   0.826ᵃᵇᵈ      0.292ᵃᵇᶜᵈ       0.975ᵃᵇᵈ
f    hybrid-kf            0.650ᵃᵇᶜᵈ   0.795ᵃᵇᵈ      0.257ᵃᵇᶜᵈ       0.925ᵇᵈ
g    hybrid-rrf           0.683ᵃᵇᶜᵈᶠ  0.890ᵃᵇᶜᵈᵉᶠʰ  0.270ᵃᵇᶜᵈʰ      0.975ᵃᵇᵈʰ
h    vec-bge-m3           0.637ᵃᵇᵈ    0.715ᵇᵈ       0.206ᵈ          0.850ᵈ
i    vec-kf               0.641ᵃᵇᶜᵈ   0.846ᵃᵇᵈ      0.277ᵃᵇᶜᵈ       0.950ᵇᵈ

## developer prompts: 284 questions
#    Model                NDCG@10     MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  ----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.267ᵇᵈ     0.316ᵇᵈ      0.068ᵇᵈ         0.553ᵇᵈ
b    claude-mem           0.203ᵈ      0.204ᵈ       0.043ᵈ          0.356ᵈ
c    e0-trigram           0.419ᵃᵇᵈ    0.547ᵃᵇᵈ     0.144ᵃᵇᵈ        0.806ᵃᵇᵈ
d    fts                  0.031       0.067        0.007           0.067
e    hybrid-d2            0.570ᵃᵇᶜᵈ   0.692ᵃᵇᶜᵈ    0.236ᵃᵇᶜᵈ       0.915ᵃᵇᶜᵈ
f    hybrid-kf            0.574ᵃᵇᶜᵈ   0.719ᵃᵇᶜᵈᵉ   0.234ᵃᵇᶜᵈ       0.919ᵃᵇᶜᵈ
g    hybrid-rrf           0.580ᵃᵇᶜᵈʰ  0.738ᵃᵇᶜᵈᵉʰ  0.235ᵃᵇᶜᵈ       0.915ᵃᵇᶜᵈ
h    vec-bge-m3           0.556ᵃᵇᶜᵈ   0.661ᵃᵇᶜᵈ    0.223ᵃᵇᶜᵈ       0.898ᵃᵇᶜᵈ
i    vec-kf               0.582ᵃᵇᶜᵈ   0.721ᵃᵇᶜᵈʰ   0.243ᵃᵇᶜᵈ       0.912ᵃᵇᶜᵈ

## agent searches: 22 questions
#    Model                NDCG@10      MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  -----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.476ᵇᵈ      0.600        0.137ᵈ          0.909ᵈ
b    claude-mem           0.432ᵈ       0.570        0.117           0.864ᵈ
c    e0-trigram           0.665ᵃᵇᵈ     0.799ᵇᵈ      0.215ᵃᵇᵈ        0.955ᵈ
d    fts                  0.261        0.447        0.083           0.500
e    hybrid-d2            0.752ᵃᵇᶜᵈᶠʰ  0.884ᵃᵇᵈ     0.258ᵃᵇᶜᵈ       0.955ᵈ
f    hybrid-kf            0.738ᵃᵇᶜᵈʰ   0.884ᵃᵇᵈ     0.256ᵃᵇᶜᵈ       0.955ᵈ
g    hybrid-rrf           0.734ᵃᵇᶜᵈʰ   0.893ᵃᵇᵈ     0.252ᵃᵇᵈ        1.000ᵈ
h    vec-bge-m3           0.668ᵃᵇᵈ     0.756ᵈ       0.227ᵃᵇᵈ        0.955ᵈ
i    vec-kf               0.725ᵃᵇᵈʰ    0.863ᵃᵇᵈ     0.262ᵃᵇᶜᵈʰ      1.000ᵈ

## prompts typed within 90 days: 73 questions
#    Model                NDCG@10      MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  -----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.278ᵇᵈ      0.300ᵇᵈ      0.083ᵇᵈ         0.548ᵇᵈ
b    claude-mem           0.240ᵈ       0.249ᵈ       0.064ᵈ          0.397ᵈ
c    e0-trigram           0.408ᵃᵇᵈ     0.481ᵃᵇᵈ     0.145ᵃᵇᵈ        0.781ᵃᵇᵈ
d    fts                  0.048        0.123        0.014           0.123
e    hybrid-d2            0.566ᵃᵇᶜᵈ    0.657ᵃᵇᶜᵈ    0.225ᵃᵇᶜᵈ       0.890ᵃᵇᶜᵈ
f    hybrid-kf            0.568ᵃᵇᶜᵈ    0.722ᵃᵇᶜᵈᵉ   0.220ᵃᵇᶜᵈ       0.890ᵃᵇᶜᵈ
g    hybrid-rrf           0.579ᵃᵇᶜᵈ    0.745ᵃᵇᶜᵈᵉ   0.223ᵃᵇᶜᵈ       0.918ᵃᵇᶜᵈ
h    vec-bge-m3           0.567ᵃᵇᶜᵈ    0.696ᵃᵇᶜᵈ    0.210ᵃᵇᶜᵈ       0.849ᵃᵇᵈ
i    vec-kf               0.602ᵃᵇᶜᵈᵉᶠ  0.777ᵃᵇᶜᵈᵉ   0.249ᵃᵇᶜᵈ       0.932ᵃᵇᶜᵈ

## prompts typed 90 days ago or earlier: 211 questions
#    Model                NDCG@10     MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  ----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.263ᵇᵈ     0.322ᵇᵈ      0.063ᵇᵈ         0.555ᵇᵈ
b    claude-mem           0.190ᵈ      0.188ᵈ       0.035ᵈ          0.341ᵈ
c    e0-trigram           0.422ᵃᵇᵈ    0.570ᵃᵇᵈ     0.144ᵃᵇᵈ        0.815ᵃᵇᵈ
d    fts                  0.025       0.047        0.004           0.047
e    hybrid-d2            0.571ᵃᵇᶜᵈ   0.704ᵃᵇᶜᵈ    0.240ᵃᵇᶜᵈ       0.924ᵃᵇᶜᵈ
f    hybrid-kf            0.577ᵃᵇᶜᵈ   0.718ᵃᵇᶜᵈʰ   0.238ᵃᵇᶜᵈ       0.929ᵃᵇᶜᵈ
g    hybrid-rrf           0.581ᵃᵇᶜᵈʰ  0.735ᵃᵇᶜᵈʰ   0.239ᵃᵇᶜᵈ       0.915ᵃᵇᶜᵈ
h    vec-bge-m3           0.552ᵃᵇᶜᵈ   0.650ᵃᵇᶜᵈ    0.227ᵃᵇᶜᵈ       0.915ᵃᵇᶜᵈ
i    vec-kf               0.575ᵃᵇᶜᵈ   0.702ᵃᵇᶜᵈ    0.241ᵃᵇᶜᵈ       0.905ᵃᵇᶜᵈ

## prompts, documents written before them only: 256 questions
#    Model                NDCG@10       MRR@10-l2      Recall@10-l2    Hit Rate@10-l2
---  -------------------  ------------  -------------  --------------  ----------------
a    claude-mem-nowindow  0.218ᵇᵈ       0.250ᵇᵈ        0.087ᵇᵈ         0.383ᵇᵈ
b    claude-mem           0.058ᵈ        0.072          0.023ᵈ          0.098ᵈ
c    e0-trigram           0.369ᵃᵇᵈ      0.463ᵃᵇᵈ       0.216ᵃᵇᵈ        0.652ᵃᵇᵈ
d    fts                  0.017         0.043          0.006           0.043
e    hybrid-d2            0.481ᵃᵇᶜᵈ     0.604ᵃᵇᶜᵈ      0.301ᵃᵇᶜᵈ       0.773ᵃᵇᶜᵈ
f    hybrid-kf            0.475ᵃᵇᶜᵈ     0.608ᵃᵇᶜᵈ      0.289ᵃᵇᶜᵈ       0.762ᵃᵇᶜᵈ
g    hybrid-rrf           0.541ᵃᵇᶜᵈᵉᶠⁱ  0.663ᵃᵇᶜᵈᵉᶠʰⁱ  0.348ᵃᵇᶜᵈᵉᶠⁱ    0.836ᵃᵇᶜᵈᵉᶠⁱ
h    vec-bge-m3           0.545ᵃᵇᶜᵈᵉᶠⁱ  0.564ᵃᵇᶜᵈ      0.361ᵃᵇᶜᵈᵉᶠⁱ    0.820ᵃᵇᶜᵈⁱ
i    vec-kf               0.461ᵃᵇᶜᵈ     0.589ᵃᵇᶜᵈ      0.295ᵃᵇᶜᵈ       0.750ᵃᵇᶜᵈ

## documents quoting the question removed: 306 questions
#    Model                NDCG@10     MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  ----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.281ᵇᵈ     0.328ᵇᵈ      0.078ᵇᵈ         0.575ᵇᵈ
b    claude-mem           0.218ᵈ      0.224ᵈ       0.051ᵈ          0.389ᵈ
c    e0-trigram           0.421ᵃᵇᵈ    0.542ᵃᵇᵈ     0.149ᵃᵇᵈ        0.801ᵃᵇᵈ
d    fts                  0.003       0.005        0.001           0.010
e    hybrid-d2            0.574ᵃᵇᶜᵈ   0.697ᵃᵇᶜᵈ    0.242ᵃᵇᶜᵈ       0.908ᵃᵇᶜᵈ
f    hybrid-kf            0.578ᵃᵇᶜᵈ   0.722ᵃᵇᶜᵈᵉʰ  0.240ᵃᵇᶜᵈ       0.915ᵃᵇᶜᵈ
g    hybrid-rrf           0.577ᵃᵇᶜᵈʰ  0.731ᵃᵇᶜᵈᵉʰ  0.238ᵃᵇᶜᵈ       0.918ᵃᵇᶜᵈ
h    vec-bge-m3           0.557ᵃᵇᶜᵈ   0.660ᵃᵇᶜᵈ    0.226ᵃᵇᶜᵈ       0.889ᵃᵇᶜᵈ
i    vec-kf               0.589ᵃᵇᶜᵈʰ  0.720ᵃᵇᶜᵈʰ   0.252ᵃᵇᶜᵈʰ      0.912ᵃᵇᶜᵈ

## observations only: 300 questions
#    Model                NDCG@10      MRR@10-l2    Recall@10-l2    Hit Rate@10-l2
---  -------------------  -----------  -----------  --------------  ----------------
a    claude-mem-nowindow  0.322ᵇᵈ      0.343ᵇᵈ      0.122ᵇᵈ         0.590ᵇᵈ
b    claude-mem           0.253ᵈ       0.235ᵈ       0.077ᵈ          0.400ᵈ
c    e0-trigram           0.462ᵃᵇᵈʰ    0.531ᵃᵇᵈ     0.228ᵃᵇᵈʰ       0.777ᵃᵇᵈʰ
d    fts                  0.021        0.042        0.007           0.043
e    hybrid-d2            0.610ᵃᵇᶜᵈᵍʰ  0.675ᵃᵇᶜᵈᵍʰ  0.356ᵃᵇᶜᵈᵍʰ     0.893ᵃᵇᶜᵈᵍʰ
f    hybrid-kf            0.612ᵃᵇᶜᵈᵍʰ  0.697ᵃᵇᶜᵈᵍʰ  0.353ᵃᵇᶜᵈᵍʰ     0.893ᵃᵇᶜᵈᵍʰ
g    hybrid-rrf           0.507ᵃᵇᶜᵈʰ   0.641ᵃᵇᶜᵈʰ   0.261ᵃᵇᶜᵈʰ      0.827ᵃᵇᶜᵈʰ
h    vec-bge-m3           0.299ᵇᵈ      0.472ᵃᵇᵈ     0.157ᵃᵇᵈ        0.540ᵇᵈ
i    vec-kf               0.620ᵃᵇᶜᵈᵍʰ  0.690ᵃᵇᶜᵈʰ   0.371ᵃᵇᶜᵈᵍʰ     0.893ᵃᵇᶜᵈᵍʰ
```

- Today's hybrid (`hybrid-d2`) scores above both claude-mem rows in every stratum. Overall: nDCG@10 0.583 against 0.219 (0.282 without claude-mem's 90-day window), Hit Rate@10 0.918 against 0.392. The difference is significant in every metric of every stratum except the 22 agent searches, too few for it.
- Today's FTS (`e0-trigram`, overall nDCG@10 0.436) is significantly above claude-mem with its window in every metric of every stratum, except Hit Rate in the agent searches. Against claude-mem without the window it is significant everywhere except the 40 English questions (no metric) and two metrics of the agent searches.
- claude-mem's 90-day window hurts most on old prompts (nDCG@10 0.190 with it, 0.263 without, for prompts typed 90 days ago or earlier; 0.240 and 0.278 within 90 days) and when only documents written before the prompt count (0.058 and 0.218).

## Findings

- Today's hook keeps teammate messages ("Another Claude session sent a message: <teammate-message …>") as prompts: `ENVELOPES` in src/hook.rs has `<agent-message` but not this form. Design B's capture (milestone 2) should treat it as an envelope.
