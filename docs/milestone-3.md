# Milestone 3: Curate

The plan is docs/milestone-3-plan.md. The dev measurements behind this note, and their harness, are in docs/spike/m3-dev.md and docs/eval/m3.py. Every measured result here is from the dev split, and no held-out transcript was read; the heavy-day size under Cost comes from counting the owner's transcripts of the last 90 days (counts only).

## Where it stands (2026-09-29)

- Tasks 1 to 11 are merged. The last of them are recuration (#189), the crash harness (#190), and claude's built-in plugins turned off for a curator call (#191, Claude Code 2.1.283).
- Task 12, the judge's first role ("shrink, never drop"), is behind `[summary] shrink` (#194), off by default. Its input clause passes on dev. Its recall clause fails for decisions by the rule declared before the run, 22 against 24 of 44 (below), a difference smaller than two runs of one arm showed (20 and 24); lessons and fixes are not measured. It stays off. Its second role (a veto on `decided` and `supersedes`) has no code: it waits on M3.
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

- **The line failed with the defaults until 2026-09-29.** Groq's free tier allows 1,000 requests and 200,000 tokens a day per model (console.groq.com/docs/rate-limits). The default chain gave each of its three Groq entries 800 calls a day and no token budget, so on a heavy day the curator took all of each model's tokens, not 20%. Since 2026-09-29 each Groq entry is admitted up to 40,000 tokens and 200 calls in any 24 hours (Groq counts its day as a rolling window, and so do the budgets; the token budget adds reported usage and estimates, so a last call can pass it by its own size). OpenRouter free takes at most 10 calls in 24 hours: a fifth of the 50 `:free` requests a day of an account that bought less than 10 credits (1,000 after that, openrouter.ai/docs/api/reference/limits). That is some 24 windows a day across the three Groq models. The rest of a heavy day falls to the subscriptions.
- **The subscriptions' speed is a limit too.** claude haiku takes about 100 s a window (212 calls: median 101 s, 99th percentile 187 s). The 526 windows of a heavy day with the shrink on are about 14.6 hours of calls in a row, and without the shrink about 65 hours. A day that heavy is curated over the following days, or by several entries at once.
- Paid entries: none in the default chain, so the USD 5 cap is not reached.
- With the curator's thinking off, a call took 16.6 s at the median without the shrink (nothink) and 15.6 s with it (short-2, 125 calls), so the 526 windows are about 2.3 hours of calls in a row. That is a projection: no full day was run.
- The free entries' budgets now meet the line. What else a heavy day needs is speed: the shrink on if several runs show it keeps decisions and lessons and fixes are measured (its clean pair, short-2 against whole3, failed the declared rule by two labels), and the curator's thinking off (#193, now the default).

## M3 on the dev labels: fails

The line: recall of the owner's decisions 80% or more, no overturned decision shown as current (0%), and at most 2% of compatible earlier decisions dropped. Eight arms were run eleven times (whole-2 and whole3 are one arm run twice, and so are cand1 and cand2, and carry1 and carry2), with one live entry (claude haiku), over the windows that hold a label (docs/spike/m3-dev.md has the harness and the definitions, fixed before any live number).

| Arm | Binary | Shrink | Thinking | Recall (of 44) | Recall, lasting (of 41) | Overturned, still current (of 19) | Compatible, dropped (of 25) | Owner-no records shown as decided (of 5) | Decided claims |
|---|---|---|---|---|---|---|---|---|---|
| whole | #194 + #191 | off | on | 10 | 10 | 2 | 0 | 4 | 65 |
| short | #194 + #191 | on | on | 13 | 13 | 2 | 0 | 1 | 57 |
| full | #195 (the owner's picks are the user's words) | off | on | 19 | 18 | 7 | 1 | 1 | 95 |
| nothink | #195 | off | off | 24 | 24 | 9 | 1 | 3 | 122 |
| whole-2 | main at ce805a4 (#219) | off | off | 20 | 20 | 9 | 1 | 3 | 176 |
| short-2 | main at ce805a4 (#219) | on | off | 22 | 22 | 7 | 1 | 1 | 142 |
| whole3 | main at ce805a4 (#219) | off | off | 24 | 24 | 10 | 1 | 2 | 163 |
| cand1 | main at 21eadd3 (#237) | off | off | 23 | 22 | 12 | 2 | 2 | 111 |
| cand2 | main at 21eadd3 (#237) | off | off | 21 | 20 | 11 | 1 | 1 | 164 |
| carry1 | a session's decisions carried (16ad856) | off | off | 21 | 21 | 8 | 1 | 3 | 111 |
| carry2 | a session's decisions carried (16ad856) | off | off | 21 | 19 | 10 | 3 | 4 | 126 |

Scored per decision since 2026-09-29 (the owner left the call to Claude): on a record that holds several labeled items (11 records do), a claim is the item's whose quote it shares text with, and one sharing none is every item's, as all of a record's claims were before. Re-scored, only one pair moved: d105 and d106 are one prompt ("じゃあCCSを完全削除して。あと、fccをxai oauthに対応させたい。"), both overturned by d111, and short, full and carry1 each show it overturned now (3, 8 and 9 before). "Recall, lasting" leaves out the three instructions for the moment (below); the line is read on all 44, as the owner labeled them.

whole and short cut 114 spans, and some of them shared a record, so that record's window was sent twice and the second pass retracted the first pass's claims: 1 overlap and 23 claims retracted in whole, 5 overlaps and 202 retracted in short. Their rows are kept here, but they do not measure what they were meant to. full and nothink cut 109 spans, merged so that none shares a record (#196's commit), and retracted none. whole-2 and short-2 cut 113 and 109 spans the same way, with none shared. short-2 retracted none. whole-2 retracted 22 claims, all in one span (seq 29865 to 29867, 4 windows): its first send got a prose answer for one window, the harness sent the span again, and the second answers replaced the first for the 3 windows that had one. Each window still ends with the claims of one answer. One span of whole-2 (seq 30261) was refused three times (unanchored). It holds none of the 44 labels, but it holds the earlier side of two overturn pairs (d105 and d106, both overturned by d111), which whole-2 counts as never derived rather than still current. whole3 is whole-2 run again from a clean home (review on #218): the harness no longer sends a span again once some of its windows were curated (ebe288e), so the arm is one pass, as short-2 is. It cut the same 113 spans and retracted none. It was stopped once, after 87 of the 113 spans, while it waited out the curator's cooldown with no call in flight, and went on from there (eab7397). One of its spans (seq 42955) was refused three times, and one of the two windows of another (seq 29872) answered prose and was left. Neither holds any of the 44 labels or a pair the lines count. cand1, cand2, carry1 and carry2 cut the same 113 spans, none shared, and retracted none. A span refused three times is left: none in cand1 and carry2, seq 30011 in cand2, and seq 17217 to 17219 and 30011 in carry1. Seq 30011 holds d100 and seq 17217 to 17219 holds d335, so those rows miss them. The table is `m3.py score`; the counts by kind, the drop classes and the records below were read after the fact with one-off queries over the same homes.

- **The owner's picks in `AskUserQuestion`** (#195): 11 of the 13 labels that are such picks reached `decided` in full, and none did in whole or short. That is the whole of the gain from whole to full (the typed prompts went from 10 to 8 of 28, run to run).
- **Recall by kind**, nothink: picks 10 of 13, typed prompts 14 of 28, accepted proposals quoted from the assistant's reply 0 of 3 (0 in every arm; the live pass curated only one of an acceptance's two windows, see Accepted proposals across two windows).
- **Instructions for the moment.** The owner left the call to Claude on 2026-09-29, with claude-mem as the reference. claude-mem keeps every prompt verbatim and records outcomes (what was learned, built, fixed, deployed or configured, its code mode in 13.28.0), with no rule for an ask that ends with the session; of the 12 labels a first pass took for such asks, the owner's claude-mem database holds 11 as the raw prompt, and only one of them (d335) also became an observation. The rule, fixed before scoring: a label counts unless it no longer applies in a later session. A limit with an end (d446, d217), a hand-off to a new session (d130, d360, d333: work left), a go-ahead (the decision it accepts: d462, d335, d146, d380), a standing permission and an environment fact (d427, d365) apply later; three do not (d300, d332, d351), where the earlier count said about 10. The memory need not keep those three: the window's summary and the raw record keep what was asked.
- **Overturns fail the 0% line** once recall rises: 7 and 9 of 19 in full and nothink, 10 in whole3, 12 and 11 on main at 21eadd3 (cand1, cand2), and 8 and 10 with a session's decisions carried (carry1, carry2), a difference inside what two runs of one arm differ by. The curator does write `supersedes` (28 to 59 active edges per arm), but not for these pairs. 17 of the 19 pairs are in one session and 2 across sessions (d123 to d361, d490 to d187); several share one earlier record, so one missed supersede still counts once per pair.
- **Why, read after the fact.** An earlier decision reaches a later window only as a candidate: up to 20 current claims of the repository that the full-text index finds for 64 trigrams spread over the window's text. What a session carries in is its goal, its previous window's proposals and its open items, not its decisions, so the harness's stub-covered windows do not hide it. Replaying that search for the 9 pairs left current in nothink (with the window's records standing in for its rendered text, and the index as it ended):
  - 54 to 63 of the 64 trigrams came from tool text in every later window, so the search mostly looks for words of tool output;
  - in 3 pairs (d151, d176, d142 as the earlier side) the earlier decision matched none of them and was not shown;
  - in 6 it was shown, near the bottom (rank 13 to 17, where 13 to 35 current claims matched), and the curator did not supersede it. With a repository's claims in the thousands rather than tens, a rank that low would not be shown.
- **Where the earlier decision was, from the candidates each window op lists (#237).** On main (cand1, cand2), 8 of the 12 and 8 of the 11 pairs left current had an earlier decision that the later window never showed; 4 and 3 were shown. 6 and 7 of the 8 are in one session. The search found none of their words (d153's "レビューツール" against claims on "local-review"), or it sampled 64 trigrams from a long reply around a short line ("改造fcc消して"). With a session's decisions carried into its later windows (carry1, carry2; spec 3.3), 1 and 2 were not shown: the cross-session pairs (d123 to d361 in both, d490 to d187 in carry2), which need the hybrid search (#222). The same-session earlier decisions were in the prompt (each session had at most 8 decisions before the later window, so the cap of 20 cut none) and still stayed current. carry1's other 8, by pair:
  - 6 share the earlier record with another label: seq 15531 holds d142, d143 and d144, and seq 30261 holds d105 and d106. The per-record definition counts a current claim of that record for each label. carry1's d111 window superseded d106's own decision ("fccをXAI OAuthに対応させたい"), but d105's ("CCSを完全削除する"), on the same record, stays current, so both pairs count.
  - 2 were drafted and then dropped by a gate, in both carry arms. The drafts in d177's and d156's windows were lowered to proposals (a done change quoted from tool output; the assistant's report), and a gate dropped them: "a proposal supersedes nothing settled". d156 is an accepted proposal. The harness curates both of its windows since #240; in acc1 its draft was a proposal from tool content, which a bare acceptance does not promote. d156's pair also shares seq 15531.
  - d153's window (the later side for d143, d144 and d151) superseded another claim. The decision is conditional ("…ならないなら全て消して").
  - So a prompt alone does not reach the 0% line. The shared records are a definition, which is the owner's call. A draft that stays a proposal (an assistant's report the owner has not accepted, or a change quoted from tool output) supersedes no decided claim. One the owner accepts is decided and can; since #240 the harness measures that path, and the curator drafted no acceptance as a decision (#244). The cross-session pairs need the hybrid search.
- **The compatible pair dropped** in full, nothink and the three later arms is d121 to d361: the later decision ("このプロジェクトを破棄して…") superseded 案B. By the owner's labels 案B had already been replaced by d123, which d361 overturns. It counts as a drop by the definition; it is 1 of 25, 4%, over the 2% line. In the four arms of 2026-09-29, the claim that supersedes 案B is d123's ("App + 薄いWorkers受付"), as the owner's labels have it, and the pair still counts. cand1 and carry2 also dropped d291 to d293 (a later delegation rule superseding the runner-update rule). carry2 also dropped d413 to d436: "iMac は随時利用可能" superseded "M1 iMacを用意可能…明日から", a detail, not a change.
- **Owner-no records**: the three in nothink (d71, d228, d410) are the three whole also showed. d71 is a long typed plan whose other sentences are rules; d228 and d410 are the assistant's reports of what it did, drafted as the user's decisions. Not a difference that thinking made. carry1 and carry2 showed 3 and 4, against 2 and 1 on main. d409 (a pasted review reply) was drafted as a decision in both carry arms and in neither main arm, and d228 in carry2. That is within what two runs of one arm differ by; it is watched in the next runs.
- **What is not in the window**: drafts dropped because their quote is not in the window, nothink: 263 over 120 kept answers. 195 are paraphrases, 30 match once whitespace is removed, 15 quote a tool's input, 13 quote another record, 6 join two pieces with "...", 1 matches after NFKC normalization, and 3 could not be joined to their window's answer. A whitespace-blind match would keep the 30.

### Thinking (#193 item 3)

Measured once each on the #195 binary, after the fact: there was no rule declared before, as there was for the shrink.
- Recall 24 of 44 with thinking off, 19 with it on.
- A curator call took 16.6 s at the median with thinking off (90th percentile 28.7 s), 101 s with it on (150 s). 0 answers were refused with it off, 5 with it on (2 unanchored, 2 prose, 1 shape).
- Decided claims: 122 with it off, 95 with it on.

So thinking off is at least as good here and six times faster. It is the default since #219, a PR of its own so it can be reverted alone.

### Run to run

whole-2 and whole3 are one arm run twice, on one binary with the same settings (whole3 without the resend): they recalled 20 and 24, and 12 of the 44 labels were recalled in one run and not in the other (8 one way, 4 the other). nothink, on #195 with the same settings, recalled 24 too. So a difference of a few labels between two single runs is not read as an effect: comparing arms needs several runs of each. short-2 is the defaults as they would ship (shrink on, thinking off), on main before #223's search change.

### Accepted proposals across two windows (#240)

- **The live pass curated one side.** Every owner-yes accepted proposal whose proposal and acceptance are two records has them in two adjacent windows: d107, d200, d462, d335, d46 and d315. The pass curated only the window of the labeled record; the other stayed a stub window with no claims. And a recuration's proposals were not carried into the session's next window (#240, fixed in #243). So no arm could promote one.
- **Harness change**, a new version: the arm below is not a row of the table. `item_records` names both ends of an accepted proposal:
  - a label on the acceptance (a prompt) counts the reply right before it, as before;
  - a label on the proposal (a reply) now also counts the owner's next prompt.
  - `score` counts a claim quoting either end, and `spans` curates both windows: a full pass has 127 spans, 113 before. `live --accepted` curates only these, 35 spans.
  - Either neighbour is taken as the acceptance without a check, so #228 item 3 applies to both ends.
- **acc1**: `live --accepted` on #243's binary at e8c7cc2, 156,580 estimated tokens. #243's later commits change only recurations cut another way than the stub, and every live window here has a stub window's exact range. Two spans answered nothing anchored once and were curated on the second try.
  - d490, a pick in `AskUserQuestion`, is one record: current.
  - The carried proposal reaches the acceptance's window: d46's acceptance supersedes it across the two windows.
  - None of the six two-window labels is current. By label, from the kept answers:
    - d107: the reply numbered three options and the owner typed "１". The curator answered no claim: the carried line is the claim's body, without the option's number.
    - d462 ("問題なければ削減作業やって") and d46 ("cloudflared の digest を新しいものへ意図的に貼り替える"): drafted as open items the user proposed, not decisions.
    - d335 ("進めて ultrathink") and d200 (the next prompt asks for another change): not drafted.
    - d315 ("モデルを3.5じゃなくて3.6の方が良くない？"): drafted as a done change and lowered, since the turn asks ("done needs the user's words or a passing run").
  - The curator's drafting stops them, and, as acc2 showed, the carrying too (#244).
- **acc2**: 9c23129 (the carried line shows the reply's line it was quoted from, "(from: …)", and the prompt names a go-ahead as the user's decision and as a supersedes). 35 spans, 158,113 estimated tokens, all curated on the first try. None of the six is current.
  - d107 answered no claim again. d462 was drafted as the user's proposal with the session goal's words, superseding nothing. d46's acceptance superseded the carried proposal but quoted a tool line in p-cipher-v2, and the gates take out a supersedes across repositories.
  - The six acceptance windows curated again on a copy, with the eval wrapper now keeping each prompt: the carried lines stop at about 1,000 to 1,300 characters (half of a fifth of the window's tokens), and results the gates had lowered to proposed came first (d462: three prices; d46: four maintenance results), so the proposal the owner answered was cut. A quote from a tool line showed its line as JSON.
- **acc3**: 819da5e (no repo fact carried, no tool line shown again). 35 spans, 157,047 estimated tokens. None of the six is current, but three acceptances supersede the proposal they answer: d462 and d46 drafted `proposed` by the curator, d315 drafted `decided` and lowered by the gates since the turn asks. In d46's prompt the cloudflared proposal was still cut from the carried lines, behind inferred results; it came as a candidate.
- **acc4**: 76d45a2 (the assistant's own proposals carried first). 35 spans, 157,326 estimated tokens. d462 is current: "問題なければ削減作業やって" drafted `decided`, superseding the carried reduction plan.
  - Every acceptance window's prompt now carries the proposal the owner answered: d107's OmniRoute option, d462's reduction plan, d335's order of the plans, and all four of d46's items.
  - The other five, by label:
    - d107: no claim; "１" is shorter than a quote may be (5 to 200 characters).
    - d335: "進めて ultrathink" not drafted.
    - d46: the curator quoted a tool line in p-cipher-v2 again, so its supersedes was taken out.
    - d315: drafted `done` and lowered, since the turn asks.
    - d200: not drafted; the owner's next prompt asks for another change, so nothing in the window accepts.
  - One sample per arm: d462 was `proposed` in acc3 and `decided` in acc4, and d46's supersedes was kept in acc3 and taken out in acc4, so 1 of 6 against 0 of 6 is within run-to-run noise. What the arms show for certain is in the kept prompts: the proposal now reaches the acceptance's window.
- **acc5**: 9d99012, the change of 30529f4 built on #247's earlier head (a line under the quote's 5-character minimum is quoted whole, such as a bare "1"). 35 spans, 157,787 estimated tokens. d462 is current again. d107 still has no claim: its acceptance window carried the OmniRoute proposal quoted from the reply's recommendation ("私の提案は、まず…"), not from its "1." line, so no number showed, and the curator summarized "１" as a digit typed alone.
  - d107's window curated again with acc6's binary (below) on four fresh copies of acc5: each time "１" was drafted as the user's decision superseding the OmniRoute proposal (one answer also superseded the key-rotation open item carried beside it). Curating it again on one copy hid the proposal the earlier answer had accepted (#249), so each repeat used a fresh copy.
- **acc6**: 2af85f9, the change of 5e209e2 built on #247's final head (a window that opens with the developer's answer to a reply carries that reply's option lines, at most 6 of 80 characters; #252 then capped them at 4 of 60, which leaves d107's three options as they were). 35 spans, 157,478 estimated tokens. d107 and d462 are current: 2 of the 6 two-window labels, against 1 in acc4 and acc5; d490 too.
  - The options line was carried into 4 of 38 prompts: d107's "１", "1. b" answering a reply's numbered questions, "うん" after a reply's numbered headings, and a request after a reply's numbered setup steps.
  - d335 and d46: the acceptance was drafted `decided` as the user's, with no supersedes, and lowered by the gates: its quote was not on the owner's line. d200's drafts were lowered as before; d315's were dropped, their quote not in the window.
  - Codex on #252: a pick was the user's own words, so it could supersede any claim of its repository. In acc6 d107's "１" superseded the reply's key-rotation lesson and CCS-removal change with the OmniRoute proposal. A pick (an option label alone: "1", "１", "②", "B", "1番で", "案A") is now bare, as a "yes" is: it settles only an option that the reply it answers listed, in the window or in the options line carried in; it replaces only a claim of its kind; it never marks done, retracts or retires a lesson. MUST-M4's provenance fallback is not applied to a pick: a bare "yes" to a reply that followed a tool call settles nothing, but a pick names one option, and d107's reply followed a tool call. Reverting that is one predicate in `gates::picks_an_option`.
  - d107's window curated again with that build (0de90bd) on four more fresh copies of acc5: each time "１" was kept as the user's decision superseding the OmniRoute proposal only.

### What the next M3 run needs

1. The test labels, classed by the same rule (instructions for the moment, above).
2. Supersedes: a window now carries its sessions' decided claims, so a same-session earlier decision is in the prompt; the cross-session pairs need the hybrid search (#222). A record holding two labels is scored per decision (2026-09-29). A draft that stays a proposal (an unaccepted assistant's report, or a change quoted from tool output) supersedes no decided claim, and one the owner accepts is decided and can. For d177's price list, the prompt can ask the curator to quote the owner's words. The accepted path is measured since #240 (`live --accepted`): the proposal reaches the acceptance's window (acc4), and with the reply's options carried, 2 of 6 acceptances settle their proposal (acc6: d462 and d107; #244: a go-ahead is often drafted as a proposal or not at all, or quotes another line than the owner's, and a quote from another repository's tool line loses its supersedes).
3. The quote match: whitespace-blind, then the paraphrases.
4. The defaults as they would ship were run once (short-2): 22 of 44.
5. Several runs of each arm (Run to run, above).

## Judge, role (a): the shrink: fails on decisions by the declared rule; off

- **Input**: 70.9% less (17.59M against 60.43M estimated tokens on the dev set), past the 30% the spec asks.
- **Recall of decisions**: 22 of 44 with the shrink (short-2) against 24 without it (whole3), on the same binary with merged spans, each one pass. The rule declared before the run was that one label's difference is noise and two or more fewer fail; the shrink recalled two fewer, so it fails. The same arm without the shrink recalled 20 in its first run (whole-2), so two runs of one arm differed by more than this pair does, and one run each cannot show that the shrink loses decisions either. whole-2 is not used for the verdict: it sent one span again after a partial answer (review on #218). The first pair (whole and short) is not used: their spans overlapped.
- **The other lines, same pair**: overturned and still current 7 against 10 of 19, compatible dropped 1 against 1 of 25, owner-no records shown as decided 1 against 2 of 5.
- **Lessons and fixes** are named by the clause too and have no dev labels. The shrink stays off by default: it failed on decisions here, and lessons and fixes are not measured (review on #220).

## Window

Not measured. The window is the smallest size that passes M2, M3 and M6 on the dev transcripts, and no size passes M3 yet: it is measured with the next M3 run.

## What needs the owner

1. M3's test labels (plan, "What needs the owner", item 1). The owner's answers of 2026-09-29 settled the rest: instructions for the moment and per-decision scoring are Claude's call (above), OpenCodeReview runs when a PR opens and on its last push (#250), and Mistral leaves the default chain (#251).
