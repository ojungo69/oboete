# Memory found by meaning at each prompt (G23)

Plan of the slice that widens what reaches the agent at each prompt, written before the code, as
the other slices' notes are. Decision 40 widened delivery at each prompt to cards and session
summaries found by meaning (G23 of the 13.34.2 comparison); decision 44 (2026-10-13, below)
settles when and how.

## What claude-mem does (13.37.0)

- At each prompt its hook (`hook claude-code session-init`) asks its always-on worker over local
  HTTP: `/api/sessions/init`, then `/api/context/semantic` with the prompt as the query
  (`src/cli/handlers/session-init.ts`, `src/services/worker/http/routes/SearchRoutes.ts`).
- A prompt under 20 characters gets nothing. Otherwise the worker searches the project's
  observations (its cards) by meaning and returns the first 5 (`limit`, at most 20), each as its
  title, date and narrative under "Relevant Past Work (semantic match)". There is no relevance
  cut: a project with observations always gets five. Summaries are not searched.
- The two calls share one budget inside the host's 15 s hook timeout (10 s by default on POSIX,
  14 s at most, 500 ms kept back; 2 s for Codex). A slow or stopped worker adds nothing.
- `CLAUDE_MEM_SEMANTIC_INJECT` is off by default; the owner has it on.

## What oboete does now

Spec 4.2 and 4.6: with `[inject] per_prompt` on (off by default), the prompt hook matches the
prompt's words (trigrams) against the session's shortlist of about 50 delivered claims, which the
worker builds ahead (with vectors where it has them), and injects the claims whose share of the
prompt's words reaches 0.53 (`shortlist::THRESHOLD`), under "Decisions that may bear on this
prompt". Claims only; imported history is never injected (4.5); the hook never embeds a query and
never waits on the network (4.1, decision 38's "Not now" for asking the resident worker).

## The change

1. **When.** At prompt submit, at the prompt point where the per-prompt block is built now (the
   agents' UserPromptSubmit, agy's PreInvocation; Grok's UserPromptSubmit keeps its picks for its
   next tool call, as now). A prompt under 20 characters, or a harness envelope, asks nothing.
2. **Who computes.** The resident viewer, which the resident worker keeps up, asked by the hook:
   `POST /api/prompt-context` on `127.0.0.1` at the configured port, with the token of
   `state/view-token`, only when the viewer holds `state/view.lock` and its outcome says it
   listens (`view::resident_up`). The request names the calling checkout's repository (and
   branch), as the hook labels it: the viewer has no checkout of its own and its other searches
   read every repository, so the repository is the search's scope and `Query::caller`, and a
   repository excluded from sending (spec 5.5) or never recorded (6.1) has its prompt embedded by
   no provider and gets the words side only. The hook loads no model and calls nothing off the
   machine. The viewer runs the search as it runs one for the page or the MCP server
   (`search::b`'s `Caller::Reader`): it embeds the prompt with the embedder set up (the local
   model, loaded on the first query and kept, on this PC; Workers AI on the iMac) through the
   egress gate, waits for that vector at most `QUERY_TIMEOUT` (1.2 s) and answers from words
   alone past it.
3. **What.** The repository's delivered claims, current cards, session summaries and imported
   history (decision 44's second answer), ranked as search ranks them: meaning and words fused
   (reciprocal rank fusion, spec 4.10; nDCG@10 0.545 against claude-mem's 0.244 on the owner's
   questions, decision 36). At most 5 (claude-mem's number), within `per_prompt_chars`, each as
   one dated line: a claim as now, a card by its title and the first sentence of its narrative, a
   summary by its request, an imported record labelled imported with its status unknown. Nothing already
   shown in the session (the shown set, as now). The prompt's own text is never injected.
4. **Below the threshold, nothing.** Each candidate passes on either side: its words' share of
   the prompt at today's 0.53, or its meaning's cosine to the prompt at `T`. Unlike claude-mem, a
   prompt nothing bears on gets nothing: text that does not bear on the prompt costs the agent's
   context. `T` before the one evaluation of decision 34 is set without model calls, from the
   Inject measurement's 50 no-answer dev questions (milestone 4 D18; nothing in the dev home
   answers them, so anything injected for one is irrelevant): the lowest `T` at which they pass
   D18's line (a one-sided 95% upper bound of 10% irrelevant: at most 1 of the 50 gets anything).
   The one evaluation sets both thresholds, with the false-premise questions, by the same line.
   `[inject] per_prompt` stays off by default until then (4.6); the owner turns it on, as the
   owner has claude-mem's on.
5. **One deadline for the prompt point.** `[inject] per_prompt_wait_ms` bounds the hook's wait
   for the viewer, counted from the hook's read fence, not from the request, so the time the
   words side takes counts against it. The hook builds the words side (as now but over the same
   kinds: the full-text index of claims, cards, summaries and imports, with the same 0.53 cut)
   while it waits for the viewer, so a late or missing answer leaves a block that is already
   built. The hook's own reads are local and not cut at the deadline, as the prompt point's reads
   were not before; they are measured (As built). The default comes from measurement, before the
   code merges: the prompt point's time (the fence, the viewer's answer with its query vector
   within 1.2 s, the search, the lines) on this PC's WSL with the local model, set so that its
   95th percentile fits, and not above claude-mem's 2 s for Codex unless the measurement says so.
   The iMac's measurement waits for resident mode there (#506). No resident viewer, no answer
   by the deadline, or an error: the words side's block. A meaning answer that comes late is lost
   for that prompt; this is the known cost.
6. **Fail-open.** Nothing on this path waits for the viewer past the deadline, and it writes no
   memory and no index: the answer is read, gated (`outbound`, as every block), fenced and shown.
   What it writes is what a search and a prompt point write now: a query sent to a provider is
   reserved and settled in `providers.db` (the embedding budget), and what is shown joins the
   session's shown set, so it is not shown again. The hook latency lines (milestone 2's write
   line, milestone 4's read line) are measured again with it.

## As built (the first slice)

- **What.** Claims, cards and session summaries. Imported history waits for the next slice: #506
  wants it only through an explicit mapping of the import's projects to repositories, and the
  import reads a project by its name (docs/claude-mem-import.md I5).
- **What is sent.** Only a prompt this call recorded with its text (a repository never recorded,
  or `store_prompts = false`, keeps its prompt), of 20 characters or more, and only to a viewer
  that holds its lock and says it listens on the configured port (`view::resident_answering`).
  The request names the repository; the viewer has no use for the branch.
- **The viewer** (`search::b::prompt_near`) embeds as a search does and answers keys and cosines
  only, the 20 nearest: delivered claims (under `DECIDED_WHERE` and with no link ending them, as
  `claims::delivered_one` reads them, before anything is kept), cards
  and summaries at `T` or above. The hook reads each through its reader and gates its line. No
  reranker or judge exists in the code; this route calls neither, and #336's, once built, stay
  out of it (spec 4.3). The route takes the viewer's token as every route does but no Origin: the
  hook is no browser. It reads as the page's search does (a GET with the token and no Origin), a
  page of another site cannot send the token header (a custom header needs a preflight, and
  OPTIONS is 405), and it leaves the page `--open` handed the browser in place.
- **Order.** Decisions first, as before (`shortlist::PLACES` units, from the shortlist or
  `delivered_ranked`, with the claims the viewer found), then cards and summaries: the words list
  (by the share of the prompt's trigrams their line holds) and the meaning list (by cosine) fused
  by reciprocal rank (`search::b::rrf_lists`), 5 places in all. A short prompt still gets the
  claims its words match, as before (`shortlist::MIN_GRAMS`). Each line is dated with the
  document's ID, which `get` takes: `- <date> <type> card <id>: <title>: <first sentence>` (a
  summary's card, whose title is that sentence, once), `- <date> session summary <id>:
  <request>`; the block is "Memory that may bear on this prompt".
- **Shown set.** A card's or a summary's entry carries `"kind"`, and the correction pass skips it
  (#506 item 1). The cards and summaries the session start's block showed join the set too, so a
  prompt does not show them again; a compaction's manifest replaces the set, as before. Grok's turn
  keeps each pick with its kind (a list of uids kept before reads as claims).
- **`T` = 0.67**, measured without model calls on 2026-10-14 with the local bge-m3 (`oboete
  embed-spike`, the production loader and gate): the 50 no-answer dev questions against every
  active claim of their repository in the dev home (5,306), the lowest cosine at which at most 1 of
  the 50 gets anything (1 does, at 0.68). Against the delivered claims alone (53) it would be 0.64,
  too few to stand for a store's cards and summaries. At 0.67, 22 of the 42 false-premise
  questions with claims in their repository get one, which the one evaluation judges.
- **The wait** defaults to 1,000 ms, measured on 2026-10-14 on this PC's WSL: a copy of the dev
  home (5,307 claims and 2,691 cards embedded by the local bge-m3), the resident viewer warm, 47
  questions of the dev pool sent as the hook sends them, with claude-mem's own processes running
  as they do. The viewer's answer: p50 70 ms, p95 88 ms, max 101 ms. The whole prompt hook: p50
  86 ms, p95 103 ms, max 116 ms with it; p50 24 ms, p95 37 ms, max 71 ms with the words side
  alone; p50 19 ms, p95 24 ms on `main` before this change. A model the viewer dropped loads in
  about 2 s, the viewer answering at once without a vector meanwhile. The default leaves ten
  times the 95th percentile, for a busier machine and a larger store (the bit index's scan grows
  with it), and stays under claude-mem's 2 s for Codex.
- **Doctor and the page.** Doctor's per-prompt line says whether a resident viewer answers now, and
  how the last prompt that could ask took its block (`state/prompt-side`: in time, after the wait,
  no viewer, or the viewer's reason for no vector); the settings page shows the same under the
  wait, in both languages.
- **The reader's model.** The viewer drops its model after 10 minutes without a search
  (`READER_IDLE`) and loads it again on the next query in about 2 s, past `QUERY_TIMEOUT`: the
  first prompt after such a pause takes words alone and starts the load, and the next ones take
  meaning. Keeping the model loaded would hold about 2.5 GB at all times.

## Where it does not reach yet

- The resident worker and viewer are Linux only for now (`view::resident_home`). On the iMac the
  block is the words side alone until resident mode is built there; doctor says so.
- Imported history, until the import maps its projects to repositories explicitly (#506).
- A Workers AI embedding is made for each prompt that asks (the iMac, once resident): it counts
  in the embedding budget and its daily cap, as search queries do, and doctor shows the count.

## Decision 44

Spec decision 44: at prompt submit the hook asks the resident viewer (replacing decision 38's
"Not now" for it); imported history may appear in the per-prompt block (an exception to 4.5);
cards, summaries and imported history join claims, each above the threshold, and the block is
empty when nothing passes.

## Spec changes

With this note: decision 44, and pointers to it in 4.1, 4.5 and 4.6. With the code: 4.2 (the
prompt hook's steps), 4.6 (the two-sided threshold and `T`'s provisional calibration), the
`[inject] per_prompt_wait_ms` key, the parity table (row 31: per-prompt hints), and the settings
page's injection section (the wait, and which side answered last).

## Tests

As built (the first slice). The three tests with a stand-in viewer run on Linux only, where the
resident viewer keeps its token (`keyfile::private_fs`, #281):

- The hook asks only a viewer that is up, with the checkout's repository; what it finds near in
  meaning joins the block; a claim it names that is not delivered stays out
  (`hook::tests::the_prompt_point_takes_the_resident_viewers_meaning_side`).
- An answer past the wait: the words side's block, within the wait
  (`a_late_answer_leaves_the_words_side_within_the_wait`).
- A prompt not recorded with its text, or under 20 characters, is sent nowhere
  (`a_prompt_not_recorded_whole_or_too_short_asks_nothing`).
- No resident viewer: the words side, over cards and summaries too, after the decisions, each line
  dated with its ID, joining the shown set with its kind (`a_prompt_gets_the_cards_and_summaries_its_words_bear_on`);
  the correction pass names neither (`a_card_shown_at_a_prompt_draws_no_correction`); what the
  session start showed is not shown again, and a summary's card shows its sentence once
  (`a_card_the_session_start_showed_is_not_shown_again`); a store without the documents' indexes
  still gets its decisions (`a_store_without_the_document_indexes_still_gets_its_decisions`).
- The viewer keeps delivered claims only, a decided lesson a later one ended among those left
  out, before anything is kept, and documents at `T`
  (`search::b::tests::prompt_near_keeps_delivered_claims_and_near_documents`); an excluded
  repository's prompt goes to no remote embedder (`prompt_near_sends_no_prompt_of_an_excluded_repository`);
  the route takes the token and no Origin and leaves the opener page
  (`view::tests::the_prompt_context_route_takes_the_token_but_no_origin`).
- The wait on the settings page and in config.toml, kept when an older page omits it
  (`settings::tests::inject_settings_check_ranges_and_save_alone`).
- The measurements above: `T` from the no-answer questions; the viewer's answer and the prompt
  hook's time on this PC's WSL.
