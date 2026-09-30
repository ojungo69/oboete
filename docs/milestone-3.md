# Milestone 3: Curate

The plan is docs/milestone-3-plan.md. The dev measurements behind this note, and their harness, are in docs/spike/m3-dev.md and docs/eval/m3.py. Every measured result here is from the dev split, and no held-out transcript was read; the heavy-day size under Cost comes from counting the owner's transcripts of the last 90 days (counts only).

## Where it stands (2026-09-30)

- Tasks 1 to 11 are merged. The last of them are recuration (#189), the crash harness (#190), and claude's built-in plugins turned off for a curator call (#191, Claude Code 2.1.283).
- Task 12, the judge's first role ("shrink, never drop"), is behind `[summary] shrink` (#194), off by default. Its input clause passes on dev. Its recall clause fails for decisions by the rule declared before the run, 22 against 24 of 44 (below), a difference smaller than two runs of one arm showed (20 and 24); lessons and fixes are not measured. It stays off. Its second role (a veto on `decided` and `supersedes`) has no code: it waits on M3.
- Task 13 has the lines below: M2 passes, Cost's Groq and OpenRouter budgets meet the line since 2026-09-29 but NIM's cannot be checked against it (NIM publishes no daily cap, below), a heavy day needs speed too, and M3 fails on the dev labels. The first dev tuning of the gates is merged: the owner's answer to `AskUserQuestion` is the owner's words (#195, #202, #209).
- M3's typed decisions: #254's prompt shipped. #259 (the typed lines listed again), #262 (a request carried out is the developer's decision) and #271 (the list judged in place, or each typed line weighed before drafting) did not ship (below).
- Since 2026-09-30 a done claim that quotes the owner's request counts as recalling the decision (the owner's answer to #262's question). Every arm rose by 0 to 5 labels, the best to 26 of 44 (carry2), and no conclusion changed (Counted again, below).
- A prompt another agent sent is not the owner's since #275 (#273). On the typed set, owner-no records decided went from 13 to 0 over 4 passes, and the typed hits stayed within noise (A prompt another agent sent, below).
- Overturns: #278 (each carried decision weighed in the curator's answer) did not ship: 10 of 19 stayed current in both arms. #286 found that the model is not the lever: with sonnet as the curator, 8 and 7 stayed current. The default curator stays Haiku, and the next arm is structural (below).

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

- **The line failed with the defaults until 2026-09-29.** Groq's free tier allows 1,000 requests and 200,000 tokens a day per model (console.groq.com/docs/rate-limits). The default chain gave each of its three Groq entries 800 calls a day and no token budget, so on a heavy day the curator took all of each model's tokens, not 20%. Since 2026-09-29 each Groq entry is admitted up to 40,000 tokens and 200 calls in any 24 hours (Groq counts its day as a rolling window, and so do the budgets; the token budget adds reported usage and estimates, so a last call can pass it by its own size). OpenRouter free takes a fifth of its key's own free-model limit, which `GET /api/v1/key` reports (#238): 10 calls in 24 hours on an account that bought less than 10 credits (50 `:free` requests a day), 200 on one that bought more (1,000 a day, openrouter.ai/docs/api/reference/limits), and 10 until the key's limit is read. That is some 24 windows a day across the three Groq models. The rest of a heavy day falls to the subscriptions.
- **NIM's budget cannot be checked against the line.** The default chain gives NIM 500 calls a day. NVIDIA publishes no daily cap for its free endpoints, only a rate of about 40 requests a minute per account that depends on the model and the traffic (NVIDIA's forum, 2026-04-29, forums.developer.nvidia.com/t/368420); users' posts there mention 1,000 free credits, which NVIDIA's pages do not state. The owner's store holds 36 NIM calls in all (2026-09-26 to 09-28), two refused with HTTP 429: too few to show a cap.
- **The subscriptions' speed is a limit too.** claude haiku takes about 100 s a window (212 calls: median 101 s, 99th percentile 187 s). The 526 windows of a heavy day with the shrink on are about 14.6 hours of calls in a row, and without the shrink about 65 hours. A day that heavy is curated over the following days, or by several entries at once.
- Paid entries: none in the default chain, so the USD 5 cap is not reached.
- With the curator's thinking off, a call took 16.6 s at the median without the shrink (nothink) and 15.6 s with it (short-2, 125 calls), so the 526 windows are about 2.3 hours of calls in a row. That is a projection: no full day was run.
- Groq's and OpenRouter's budgets now meet the line, and NIM's is not known against it (above). What else a heavy day needs is speed: the shrink on if several runs show it keeps decisions and lessons and fixes are measured (its clean pair, short-2 against whole3, failed the declared rule by two labels), and the curator's thinking off (#193, now the default).

## M3 on the dev labels: fails

The line: recall of the owner's decisions 80% or more, no overturned decision shown as current (0%), and at most 2% of compatible earlier decisions dropped. Eight arms were run eleven times (whole-2 and whole3 are one arm run twice, and so are cand1 and cand2, and carry1 and carry2), with one live entry (claude haiku), over the windows that hold a label (docs/spike/m3-dev.md has the harness and the definitions, fixed before any live number). The table counts claims with status decided only, as every arm was scored until 2026-09-30; Counted again, below, has each arm with a done claim quoting the owner's request counted too.

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

whole and short cut 114 spans, and some of them shared a record, so that record's window was sent twice and the second pass retracted the first pass's claims: 1 overlap and 23 claims retracted in whole, 5 overlaps and 202 retracted in short. Their rows are kept here, but they do not measure what they were meant to. full and nothink cut 109 spans, merged so that none shares a record (#196's commit), and retracted none. whole-2 and short-2 cut 113 and 109 spans the same way, with none shared. short-2 retracted none. whole-2 retracted 22 claims, all in one span (seq 29865 to 29867, 4 windows): its first send got a prose answer for one window, the harness sent the span again, and the second answers replaced the first for the 3 windows that had one. Each window still ends with the claims of one answer. One span of whole-2 (seq 30261) was refused three times (unanchored). It holds none of the 44 labels, but it holds the earlier side of two overturn pairs (d105 and d106, both overturned by d111), which whole-2 counts as never derived rather than still current. whole3 is whole-2 run again from a clean home (review on #218): the harness no longer sends a span again once some of its windows were curated (ebe288e), so the arm is one pass, as short-2 is. It cut the same 113 spans and retracted none. It was stopped once, after 87 of the 113 spans, while it waited out the curator's cooldown with no call in flight, and went on from there (eab7397). One of its spans (seq 42955) was refused three times, and one of the two windows of another (seq 29872) answered prose and was left. Neither holds any of the 44 labels or a pair the lines count. cand1, cand2, carry1 and carry2 cut the same 113 spans, none shared, and retracted none. A span refused three times is left: none in cand1 and carry2, seq 30011 in cand2, and seq 17217 to 17219 and 30011 in carry1. Seq 30011 holds d100 and seq 17217 to 17219 holds d335, so those rows miss them. The table is `m3.py score` as it was before 2026-09-30 (each home keeps that score as `score-decided-only.json`); the counts by kind, the drop classes and the records below were read after the fact with one-off queries over the same homes.

- **The owner's picks in `AskUserQuestion`** (#195): 11 of the 13 labels that are such picks reached `decided` in full, and none did in whole or short. That is the whole of the gain from whole to full (the typed prompts went from 10 to 8 of 28, run to run).
- **Recall by kind**, nothink: picks 10 of 13, typed prompts 14 of 28, accepted proposals quoted from the assistant's reply 0 of 3 (0 in every arm; the live pass curated only one of an acceptance's two windows, see Accepted proposals across two windows).
- **Instructions for the moment.** The owner left the call to Claude on 2026-09-29, with claude-mem as the reference. claude-mem keeps every prompt verbatim and records outcomes (what was learned, built, fixed, deployed or configured, its code mode in 13.28.0), with no rule for an ask that ends with the session. A first pass took 13 of the 44 labels for such asks; of the 12 searched in the owner's claude-mem database, 11 are there as the raw prompt, and only one of them (d335) also became an observation. The rule, fixed before scoring: a label counts unless it no longer applies in a later session. A limit with an end (d446, d217), a hand-off to a new session (d130, d360, d333: work left), a go-ahead (the decision it accepts: d462, d335, d146, d380), a standing permission and an environment fact (d427, d365) apply later; three do not (d300, d332, d351), where the earlier count said about 10. The memory need not keep those three: the window's summary and the raw record keep what was asked.
- **Overturns fail the 0% line** once recall rises: 7 and 9 of 19 in full and nothink, 10 in whole3, 12 and 11 on main at 21eadd3 (cand1, cand2), and 8 and 10 with a session's decisions carried (carry1, carry2), a difference inside what two runs of one arm differ by. The curator does write `supersedes` (28 to 59 active edges per arm), but not for these pairs. 17 of the 19 pairs are in one session and 2 across sessions (d123 to d361, d490 to d187); several share one earlier record, so one missed supersede still counts once per pair.
- **Why, read after the fact.** An earlier decision reaches a later window only as a candidate: up to 20 current claims of the repository that the full-text index finds for 64 trigrams spread over the window's text. What a session carries in is its goal, its previous window's proposals and its open items, not its decisions, so the harness's stub-covered windows do not hide it. Replaying that search for the 9 pairs left current in nothink (with the window's records standing in for its rendered text, and the index as it ended):
  - 54 to 63 of the 64 trigrams came from tool text in every later window, so the search mostly looks for words of tool output;
  - in 3 pairs (d151, d176, d142 as the earlier side) the earlier decision matched none of them and was not shown;
  - in 6 it was shown, near the bottom (rank 13 to 17, where 13 to 35 current claims matched), and the curator did not supersede it. With a repository's claims in the thousands rather than tens, a rank that low would not be shown.
- **Where the earlier decision was, from the candidates each window op lists (#237).** On main (cand1, cand2), 8 of the 12 and 8 of the 11 pairs left current had an earlier decision that the later window never showed; 4 and 3 were shown. 6 and 7 of the 8 are in one session. The search found none of their words (d153's "レビューツール" against claims on "local-review"), or it sampled 64 trigrams from a long reply around a short line ("改造fcc消して"). With a session's decisions carried into its later windows (carry1, carry2; spec 3.3), 1 and 2 were not shown: the cross-session pairs (d123 to d361 in both, d490 to d187 in carry2), which need the hybrid search (#222). The same-session earlier decisions were in the prompt (each session had at most 8 decisions before the later window, so the cap of 20 cut none) and still stayed current. carry1's other 7, by pair:
  - 5 share the earlier record with another label: seq 15531 holds d142, d143 and d144, and seq 30261 holds d105 and d106. Scored per decision, each label counts its own claims: carry1's d111 window superseded d106's own decision ("fccをXAI OAuthに対応させたい"), which counts as overturned now, and d105's ("CCSを完全削除する"), on the same record, stays current. Per record, both pairs counted as current (9 before).
  - 2 were drafted and then dropped by a gate, in both carry arms. The drafts in d177's and d156's windows were lowered to proposals (a done change quoted from tool output; the assistant's report), and a gate dropped them: "a proposal supersedes nothing settled". d156 is an accepted proposal. The harness curates both of its windows since #240; in acc1 its draft was a proposal from tool content, which a bare acceptance does not promote. d156's pair also shares seq 15531.
  - d153's window (the later side for d143, d144 and d151) superseded another claim. The decision is conditional ("…ならないなら全て消して").
  - So a prompt alone does not reach the 0% line. The shared records are scored per decision since 2026-09-29. A draft that stays a proposal (an assistant's report the owner has not accepted, or a change quoted from tool output) supersedes no decided claim. One the owner accepts is decided and can; since #240 the harness measures that path, and in these arms the curator drafted no acceptance as a decision (#244; the accepted-proposal arms below). The cross-session pairs need the hybrid search.
- **The compatible pair dropped** in full, nothink and the three later arms is d121 to d361: the later decision ("このプロジェクトを破棄して…") superseded 案B. By the owner's labels 案B had already been replaced by d123, which d361 overturns. It counts as a drop by the definition; it is 1 of 25, 4%, over the 2% line. In the four arms of 2026-09-29, the claim that supersedes 案B is d123's ("App + 薄いWorkers受付"), as the owner's labels have it, and the pair still counts. cand1 and carry2 also dropped d291 to d293 (a later delegation rule superseding the runner-update rule). carry2 also dropped d413 to d436: "iMac は随時利用可能" superseded "M1 iMacを用意可能…明日から", a detail, not a change.
- **Owner-no records**: the three in nothink (d71, d228, d410) are the three whole also showed. d71 is a long typed plan whose other sentences are rules; d228 and d410 are the assistant's reports of what it did, drafted as the user's decisions. Read again for #273: all three, and d409, are prompts another agent sent in a Codex session, which #275 no longer records as the owner's. Not a difference that thinking made. carry1 and carry2 showed 3 and 4, against 2 and 1 on main. d409 (a pasted review reply) was drafted as a decision in both carry arms and in neither main arm, and d228 in carry2. That is within what two runs of one arm differ by; it is watched in the next runs.
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

### Keep what applies later (#254)

The curator's prompt now says to keep what should still change what an agent does in a later session. That is the rule the labels use for instructions for the moment (above). A request that ends with the session is not a claim. A go-ahead is the decision it accepts. A rule, permission, limit or fact about the setup is kept, even when the developer states it as a request or in passing. Work left for a new session is a decided open item. The verdict was declared in #254 before the run.

- **Run**: `live --typed` (the 25 owner-yes labels in the owner's own prompts and the 5 owner-no records, 30 spans). 4 passes per arm, alternating, each from a fresh copy of the prepared home. B is `oboete-base-2ea4fa8` (#252 at 2ea4fa8) and L is `oboete-later-6da174d`, the same build with the prompt change.
- **Hits over the 24 lasting typed labels** (0 to 4 each; d332 is left out): B 48, L 56. That is +8, the verdict's threshold, so L ships. No label lost more than one hit (d435, 4 → 3). The owner-no records decided were 13 in each arm, over 5 records × 4 passes.
- **Labels that moved** (B → L):
  - d360「新セッションで実装する」: 1 → 4. The prompt names work left for a new session.
  - d215: 1 → 3.
  - d11, d112, d146, d294: +1 each.
  - d332, the instruction for the moment in the set: 1 → 0.
- **Decided claims per pass**: B 17, 31, 38, 52; L 28, 54, 22, 29. The spread within an arm is wide, but the means are close (34.5 and 33.3). With equal owner-no counts, L's gain is not from deciding more overall.

### Why typed decisions are missed, and the typed lines listed again (#259)

#254's L passes missed 40 of the 96 hits the 24 lasting typed labels could have (4 passes). By what the passes kept, and the curator's answers the harness saved:

- **No draft from the typed line** (15): d331「野良コンテナの判断は任せる」, d363「全権限をいつでも使えるように認証したいからそのコマンドを教えて」 and d365「ブラウザ開かないからurl教えて」 in all 4 passes, and d211 in 3. Each window is mostly tool output and the assistant's report, and the curator drafted repo facts from those. d365's line is also close to an example #254 added to the requests that stop mattering ("a URL to open").
- **The request, carried out in the window, kept as done** quoting the developer's line (7): d352「モデルにglm5.2を追加して」 as a change with status done 3 times, d294 and d54 as decisions with status done twice each. M3 counts a claim with status decided, of any kind.
- **A question or a condition** (8): d293「codexみたいにフル権限で委譲する方が運用しやすくない？」 lowered to a proposal twice and superseded twice; d427「CodeRabbitが必要なら待っていいよ。」 a proposal 3 times, and once a draft that quoted both of its sentences across the line break was dropped as not in the window. Both of #254's builds predate #248 (the whitespace-blind quote match).
- **A label whose quote is the assistant's text** (4): d50, in all 4 passes.
- **Once each** (6): d112, d352 and d435 with no claim, d215 and d54 a proposal, d146 unverified.

The change for the first group: the window's typed lines (`[user]` lines, not `AskUserQuestion` answers), each cut at 200 characters, are listed again at the end of the prompt, after the carried claims, and the instructions say to read each on its own. #254's example "a URL to open" is now "a link to click once".

- **Run**: `live --typed`, 4 passes per arm, alternating, each from a fresh copy of the prepared home. B is `oboete-base259-58a322c` (#256 at 3b8283f with #258's prompt, so both arms have #248) and N is `oboete-typed-e1fd962`, the same build with the change. The verdict was declared in #259 before the run.
- **Hits over the 24 lasting typed labels**: B 59, N 62. That is +3, under the +8 to ship, and d112 lost 3 hits (3 → 0), which alone rejects it. The change is not in main. The owner-no records decided were 16 and 12, d332 (the instruction for the moment) 0 and 2, and the prompt tokens 1.9% more in N.
- **What moved** (B → N):
  - The first group moved as intended: d331 1 → 4, d363 0 → 1. d294 went 0 → 4 (a decision with status done in every B pass), d146 2 → 4, d449 3 → 4.
  - d112「削除して。ついでにuranai-aiとcloudflareのuranai-ai関連も削除して」 3 → 0: in all 4 N passes the curator answered its window with no claims; B's passes drafted the line as a decision 3 times. d446 4 → 2; d215, d435 and d54 lost 1 each.
  - d365 stayed at 0 in both arms, with the example changed.
- **A reading, not tested**: "read each on its own" invites judging a line without the lines around it, so a request whose weight is in its context (d112's deletion, after a long exchange) reads as one step that stops mattering. A variant would keep the list and drop that phrase.
- **Decided claims per pass**: B 48, 30, 28, 48; N 36, 29, 33, 30.

### A request carried out is the developer's decision (#262)

The second group of #259's breakdown is a request that an agent carried out in the same window and that was kept as done. The change: the keep rules say that a change the developer asks for is a decided decision quoting their line, even when an agent then makes it, and the decision's body says that it was made. The verdict was declared in #262 before the run.

- **Run**: `live --typed`, 4 passes per arm, alternating B and N, each from a fresh copy of the prepared home. B is `oboete-base259-58a322c` (#259's B arm). N is `oboete-asked-9f72dda`, which is B with the change.
- **Hits over the 24 lasting typed labels**: B 59, N 57, a difference of −2. No label lost 3 hits; d146, d363 and d449 lost 2 each.
  - The owner-no records decided were 14 in B and 12 in N.
  - N used 1.1% more prompt tokens.
  - By the declared rule the result is inconclusive, so the change does not ship and is not in main.
- **The target did not move**:
  - d352「モデルにglm5.2を追加して」 was never decided in either arm. In N it was a change with status done in 3 passes, and a decision with status done in the fourth.
  - d294 and d331 gained a pass each.
  - d146 lost two passes, each time to a claim with status done.
  - The curator keeps a request that the window carries out as done, whatever the prompt says.
- **Run to run, 4 passes per arm**: B is #259's B binary run again, so the two B arms show how much one binary varies between runs.
  - The hits were 59 both times, but 10 of the 24 labels differ, by 14 hits in all.
  - One label moved by 3 (d363, 0 → 3).
  - So one label losing 3 hits is within run-to-run noise at 4 passes. #259's rejection rested on that rule: d112 went 3 → 0. Its sum, +3, was under its +8 in any case.
  - A rule declared for the next run should not reject on one label alone, or it needs more passes.
- **A question about the metric**: M3 counts a claim with status decided. A done claim that quotes the owner's request records that the change was asked for and made. Whether that counts as recalling the owner's decision depends on what "remembering my decision" means to the owner. The curator's prompt cannot settle it. The owner answered on 2026-09-30 (next section).

### Counted again: a request the owner made and an agent carried out (2026-09-30)

The owner's answer to #262's question: a claim with status done that quotes the owner's own words counts, since the owner asked for it and it was done. The harness now counts such a claim as recalling its label, and as shown as current where the label is overturned or an owner-no record (docs/spike/m3-dev.md). Every arm's home was scored again with it; each keeps its earlier score as `score-decided-only.json`.

| Arm | Recall (of 44) | Recall, lasting (of 41) | Overturned, still current (of 19) | Owner-no records shown as decided (of 5) | Done claims quoting the owner |
|---|---|---|---|---|---|
| whole | 11 (10) | 11 (10) | 2 | 4 | 16 |
| short | 13 | 13 | 2 | 1 | 2 |
| full | 21 (19) | 20 (18) | 8 (7) | 1 | 9 |
| nothink | 24 | 24 | 10 (9) | 4 (3) | 8 |
| whole-2 | 22 (20) | 22 (20) | 9 | 3 | 12 |
| short-2 | 22 | 22 | 7 | 3 (1) | 9 |
| whole3 | 24 | 24 | 10 | 3 (2) | 10 |
| cand1 | 25 (23) | 24 (22) | 12 | 3 (2) | 23 |
| cand2 | 23 (21) | 22 (20) | 11 | 2 (1) | 17 |
| carry1 | 22 (21) | 22 (21) | 8 | 4 (3) | 12 |
| carry2 | 26 (21) | 24 (19) | 10 | 4 | 19 |

(In brackets, the count before, where it moved. The compatible pairs dropped did not move.)

- **What moved**: recall rose by 0 to 5 labels, and the best is carry2's 26 of 44 (59%; the line is 80%). carry1 and carry2, one binary, now differ by 4 (22 and 26), and whole-2 and whole3 by 2 (22 and 24): run to run, as before.
- **The conclusions hold**:
  - M3 fails: recall 26 of 44 at best, and 7 to 12 of the 19 overturned decisions still current.
  - The shrink: short-2 22 against whole3 24, as before, so it stays off.
  - Thinking: 24 with it off (nothink), 21 with it on (full), where it was 19.
  - #259 and #262, in hits over the 24 lasting typed labels at 4 passes per arm: #259 B 69, N 68 (−1, and d112 still went 3 → 0); #262 B 68, N 67 (−1). Neither ships. d352 is now recalled in all 4 of #262's N passes and 3 of its B passes.

### Typed lines judged in place, or weighed one by one (#271)

Two answers to #254's largest group of misses (a short typed line in a window of tool output, with no draft), each against main at 2f0bea9, declared in #271 before the run:

- **C**: #259's list of the typed lines at the prompt's end, with "read each on its own" replaced by "Judge each where it stands, with the lines around it" (branch `curate/typed-context`).
- **A**: no list; the answer starts with a `typed` array, one entry per `[user]` line (its id, whether a later session should follow it, and why), which no code reads (branch `curate/typed-accounting`).

The run: 4 rounds of B, C and A, each pass from a fresh copy of the prepared home, main's harness copied once, the claude CLI on Haiku as the one curator.

| | B | C | A |
|---|---|---|---|
| Hits over the 24 lasting typed labels (0 to 4 each) | 71 | 71 | 72 |
| Owner-no records decided, 5 × 4 passes | 13 | 16 | 16 |
| Prompt tokens, 4 passes (estimate) | 626,073 | 635,010 (+1.4%) | 625,820 (−0.0%) |

- **Verdict**: inconclusive, so neither ships. The rule was +6 on the sum, at most +2 owner-no records, and at most +10% prompt tokens. C is +0 and A +1, and both are +3 on the owner-no line.
- **What moved** (B → C, B → A):
  - #254's first group, in both arms: d331「野良コンテナの判断は任せる」 1 → 4 and 1 → 3, d363 1 → 3 and 1 → 2, d365 0 → 0 and 0 → 1.
  - d54「autocompactを50%にして」 4 → 1 and 4 → 0. B's hits were a decision with status decided twice and an open item with status decided twice; C's and A's drafts were decisions with status proposed in 3 and 4 passes, each quoting the developer's own line (speaker user).
  - d112's deletion request 4 → 0 in C, as in #259's N (3 → 0), and 4 → 4 in A: the list at the prompt's end loses it, not #259's phrase.
- **A reading, not tested**: weighing a typed line on its own terms brings in short rules and permissions (d331, d363) and moves a short request carried out in the window (d54) to a proposal. A variant would need to keep the line's own status (asked and done) while it is weighed. The prompt already defines decided as what the developer said, asked for or accepted, so a decision the developer asked for, drafted as proposed with speaker user, contradicts it.

### A prompt another agent sent (#273, #275)

A Codex session that another agent starts holds that agent's prompts: a `codex exec` run, or a session Claude Code's Codex plugin starts. Until #275 capture stored them as the owner's typed lines. So a claim quoting one had speaker user and counted as the owner's decision.
- 4 of the 5 owner-no records are such prompts: d71, d228, d409 and d410. The owner said none of them is their decision, and this note had read them as a long plan, the assistant's reports and a pasted review reply.
- None of the owner-yes labels is.
- #275 marks such a prompt `agent_sent`, and curation shows it as `[agent prompt]` with the assistant's role, so a claim quoting it is never the user's.

- **Run**:
  - Arms: 4 passes of main with #275 (033d5b5) against #271's B arm (main at 2f0bea9), with the same harness copy and the claude CLI on Haiku.
  - Base: the dev base's copy `base273`. Its 47 Codex prompt records from sessions another agent started carry `agent_sent`, as #275's capture writes it; the other 86 are the owner's.
  - Why no new B arm: #275's later commits change the manifest and the live hook, not curation, and main's curation did not change between 2f0bea9 and #275.

| | B | #275 |
|---|---|---|
| Hits over the 24 lasting typed labels (0 to 4 each) | 71 | 69 |
| Owner-no records decided, 5 × 4 passes | 13 | 0 |
| Claims with speaker user, decided or done, quoting an agent's prompt | 98 | 0 |
| Decided claims per pass | 34, 41, 33, 34 | 18, 18, 18, 16 |
| Prompt tokens, 4 passes (estimate) | 626,073 | 625,658 |

- **Reading**:
  - The owner-no line is clear of the four agent prompts. The fifth owner-no record (d212, a Claude reply in the owner's own session) was decided in no pass of either arm.
  - The typed hits moved by 2, inside what two runs of one binary differ by (#262): d130, d211 and d435 lost one pass each and d215 gained one.
  - About half of the decided claims per pass in B quoted agent prompts.
- **Not measured here**: `decided` precision over all claims, which the M3 line reads on the test labels (100 claims the curator marked decided). The drop in decided claims per pass says where most of the lost precision was.
- **What stays open**:
  - A session another agent started and the owner then resumes in the TUI keeps the agent's mark, and so does a `codex exec` run the owner starts in their own terminal (#276). Neither shows in the owner's 1,755 rollouts.

### Which curator M3 is read on (2026-09-30)

Owner decision 26 reads M3's lines per curator model: the cheapest model that passes becomes the default. Every dev arm so far ran one model, Haiku through the claude CLI (a subscription), so a difference between arms is the arm's and not the chain's. The chain as it ships puts the free API entries first (owner, 2026-09-27), and its first entry, Groq's gpt-oss-120b, has no M3 number yet (Claude; overrulable):

- The dev arms keep running on Haiku.
- Before the deciding run, each entry the defaults use first gets one dev pass on the typed set and one on the pairs, and the deciding run is read for the model the defaults would then put first.
- Groq is not measured now. Its free tier allows 200,000 gpt-oss-120b tokens a day, cached ones aside (console.groq.com/docs/rate-limits, read 2026-09-30). The owner's own curation (the current binary) counted 179,516 of them in the 24 hours before 2026-09-30 11:40 JST, and 27,000 to 58,000 on each of the three days before. A dev pass is about 157,000 tokens on the typed set and 351,000 on the pairs (#278's B passes): the pairs are more than a day's allowance, and either would move the owner's curation off its first entry. Measuring it needs a paid tier or another key, which is the owner's call. (Of the owner's 48 Groq gpt-oss-120b calls that day, 37 answered and 11 failed: 9 as `json_validate_failed`, Groq's own check of the answer's JSON, one 413 and one 429; the chain went on to the next entry each time, and 7 more waited out a 429 of under a minute.)

### Carried decisions weighed one by one (#278)

Both failing lines rest on one behavior: the curator does not supersede an earlier decision that is already in its prompt. #278 asked for each one, declared before any live number:

- **Arms**: B is main at aa85a49. P is branch `curate/overturn-accounting` (5e7d75d): the answer starts with `reversed`, one entry per carried `decided before` claim, in order (its uid, whether a line of the window changes, reverses or cancels it, and that line), and `parse` adds the uid to the supersedes of the drafts from that line.
- **Run**: 4 passes of each arm over the 67 spans that hold either end of a pair the lines score (`m3.py live --pairs`), each from a fresh copy of `base273`, the claude CLI on Haiku. B and P ran side by side after B1's first 27 spans: a pass waits out the curator's cooldowns, and three unanchored answers in a row rest the claude entry for 30 minutes.
- **Rule**: adopt when mean O(P) ≤ mean O(B) − 3, mean N(P) ≤ mean N(B) + 1, and total D(P) ≤ total D(B) + 2.

| | B | P |
|---|---|---|
| Overturned, still current (O, of 19) | 9, 11, 10, 10 (mean 10) | 11, 10, 9, 10 (mean 10) |
| Overturned, never derived (N) | 0, 0, 2, 2 (mean 1) | 1, 1, 1, 2 (mean 1.25) |
| Compatible, dropped (D, of 25) | 6, 8, 8, 7 (total 29) | 11, 10, 8, 9 (total 38) |
| Supersede edges a pass (mean) | 33.5 | 51.5 |

- **Verdict**: not adopted. O did not move (10 against 10), and D rose by 9: P writes more supersede edges, and more of them drop a compatible decision.
- **What P's `reversed` said**, for the 10 pairs both arms left current in every pass or nearly (from the saved prompts and answers):
  - Named, but no edge (2 pairs): d142 → d156 and d176 → d177. `reversed` was true with the right line in 3 of 4 passes each. The drafts from that line quote the assistant's report (「`review-routing` スキルを書き換えて、外部レビューの既定を cubic にした。」) or the tool output around the owner's price list, the gates lower them to proposals, and "a proposal supersedes nothing settled" drops the edge, as in carry1 and carry2.
  - Carried, not named (4 pairs): d143, d144 and d151, which the owner's 「今後cubic cliやcoderabbit cliのように簡単に使えて高精度なレビューツールになるなら残すけど、ならないなら全て消して」 (d153) overturns. In P1 and P2 the list named none of the carried decisions: in P1 its 7 entries were the window's 7 kept claims, in P2 its 5 were 3 kept claims and 2 carried open items. In P3 and P4 it answered false for all three: the line is conditional. d143 → d154 (「ローカルもGitHubも全部消す」) is not, and was false or missing in every pass.
  - d105 and d106 → d111 (「改造fcc消して。また今度omnirouteで設定する」): carried in some passes; where P answered for them (P2), both false.
  - Not carried (2 pairs): d123 → d361 and d490 → d187, across sessions and not among the window's kept claims either. d461 → d463 also stayed current in 1 pass of B and 3 of P.
- **The uids**: of the 916 `reversed` entries over P's passes, 4 named a uid its prompt did not hold, and all 480 uids in `supersedes` over the 8 passes were in their prompts. The curator copies the 64-character uids; it applies them to the wrong claims.
- **Reading**: asking for each carried decision moves nothing that the carried prompt did not. What stays current has the shapes carry1 and carry2 showed: a reversal drafted from a report or a tool line (the gates, by design), a line the curator judges as not reversing (d153's condition, d154, d111), and the pairs across sessions (#222). #286 measures the same B with sonnet as the curator, to tell whether the model is the lever for the middle group.

### Sonnet as the curator (#286)

A diagnostic, declared in #286 before its first pass: #278's B exactly (main aa85a49, the same harness copy and base, the claude CLI with `--effort low` and thinking off, as shipped), with the live entry's model `sonnet` instead of `haiku`, over every labeled window (127 spans a pass), 2 passes. The read-out, not an adoption rule: the model is the lever if the mean of overturned decisions still current is 3 or fewer, and not the lever at 7 or more.

| | S1 | S2 |
|---|---|---|
| Overturned, still current (of 19) | 8 | 7 |
| Compatible, dropped (of 25) | 5 | 7 |
| Recall (of 44, decided or done by the owner's request) | 31 | 28 |
| Owner-no records shown as decided (of 5) | 0 | 0 |
| Decided claims | 91 | 74 |
| Median seconds a window | 7.5 | 7.4 |
| Tokens a pass (estimate) | 710,170 | 710,542 |

- **Read-out**: a mean of 7.5, so the model is not the lever, and the next arm is structural. The default curator stays Haiku.
- **Recall**: 29.5 on average, more than the latest Haiku arms on every labeled window (21 to 26, Counted again, above), below the line's 35.
- **Left current in both passes** (7): d105 and d106 → d111, d142 → d156, d143, d144 and d151 → d153, and d143 → d154; d490 → d187 also in S1. Sonnet overturned d176 → d177 (its draft quoted the owner's price list) and d123 → d361 in both passes, which Haiku left current in every pass of #278; those passes sent only the pairs' spans, and these every labeled window.
- **Speed**: Sonnet answered faster than Haiku: 7.5 s at the median a window, against 12.4 to 13.3 s in #278's B passes.

### What the next M3 run needs

1. The test labels, classed by the same rule (instructions for the moment, above).
2. Supersedes: a window now carries its sessions' decided claims, so a same-session earlier decision is in the prompt; the cross-session pairs need the hybrid search (#222). The same-session pairs that Haiku and Sonnet both leave current are d105 and d106 → d111, d142 → d156 and d143, d144 and d151 → d153 and d154 (#278, #286): neither a prompt nor a larger model moved them, so the next arm is structural, declared in its own issue before its run. A record holding two labels is scored per decision (2026-09-29). A draft that stays a proposal (an unaccepted assistant's report, or a change quoted from tool output) supersedes no decided claim, and one the owner accepts is decided and can. For d177's price list, the prompt can ask the curator to quote the owner's words. The accepted path is measured since #240 (`live --accepted`): the proposal reaches the acceptance's window (acc4), and with the reply's options carried, 2 of 6 acceptances settle their proposal (acc6: d462 and d107; #244: a go-ahead is often drafted as a proposal or not at all, or quotes another line than the owner's, and a quote from another repository's tool line loses its supersedes).
3. The quote match: the paraphrases (whitespace-blind matching is #248).
4. The defaults as they would ship were run once (short-2): 22 of 44.
5. A base whose Codex prompts from sessions another agent started carry `agent_sent` (#275): a rebuilt base gets it from the transcript parser, and the frozen dev base's copy `base273` has it (the 47 records named by their rollouts).
6. Several runs of each arm (Run to run, above). At 4 passes per arm one label can move by 3 between two runs of one binary (#262), so a declared rule should not reject on one label alone.

## Judge, role (a): the shrink: fails on decisions by the declared rule; off

- **Input**: 70.9% less (17.59M against 60.43M estimated tokens on the dev set), past the 30% the spec asks.
- **Recall of decisions**: 22 of 44 with the shrink (short-2) against 24 without it (whole3), on the same binary with merged spans, each one pass. The rule declared before the run was that one label's difference is noise and two or more fewer fail; the shrink recalled two fewer, so it fails. The same arm without the shrink recalled 20 in its first run (whole-2), so two runs of one arm differed by more than this pair does, and one run each cannot show that the shrink loses decisions either. whole-2 is not used for the verdict: it sent one span again after a partial answer (review on #218). The first pair (whole and short) is not used: their spans overlapped.
- **The other lines, same pair**: overturned and still current 7 against 10 of 19, compatible dropped 1 against 1 of 25, owner-no records shown as decided 1 against 2 of 5.
- **Lessons and fixes** are named by the clause too and have no dev labels. The shrink stays off by default: it failed on decisions here, and lessons and fixes are not measured (review on #220).

## Window

Not measured. The window is the smallest size that passes M2, M3 and M6 on the dev transcripts, and no size passes M3 yet: it is measured with the next M3 run.

## What needs the owner

1. M3's test labels (plan, "What needs the owner", item 1). The owner's answers of 2026-09-29 settled the rest: instructions for the moment and per-decision scoring are Claude's call (above), OpenCodeReview runs when a PR opens and on its last push (#250), and Mistral leaves the default chain (#251).
