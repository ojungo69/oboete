# Memory found by meaning at each prompt (G23)

Plan of the slice that widens what reaches the agent at each prompt, written before the code, as
the other slices' notes are. Decision 40 widened delivery at each prompt to cards and session
summaries found by meaning (G23 of the 13.34.2 comparison); the owner's answers of 2026-10-13
(decision 44, below) settle when and how.

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
   listens (`view::resident_up`). The hook loads no model and calls nothing off the machine. The
   viewer runs the search as it runs one for the page or the MCP server (`search::b`'s
   `Caller::Reader`): it embeds the prompt with the embedder set up (the local model, loaded on the
   first query and kept, on this PC; Workers AI on the iMac) through the egress gate, waits for
   that vector at most `QUERY_TIMEOUT` (1.2 s) and answers from words alone past it.
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
5. **A cap on the wait.** The hook waits at most `[inject] per_prompt_wait_ms`. Its default comes
   from measurement, before the code merges: the viewer's answer time (the query vector within
   its 1.2 s, the search, the lines) on the owner's two machines, this PC with the local model
   and the iMac with Workers AI, set so that the slower machine's 95th percentile fits, and not
   above claude-mem's 2 s for Codex unless the measurement says so. No resident viewer, no answer
   in time, or an error: the hook builds the block from the words side alone, as now but over the
   same kinds (the full-text index of claims, cards, summaries and imports), with the same 0.53
   cut. A meaning answer that comes late is lost for that prompt; this is the known cost.
6. **Fail-open.** Nothing on this path blocks the prompt past the cap, and nothing it does is
   written: the answer is read, gated (`outbound`, as every block), fenced and shown. The hook
   latency lines (milestone 2's write line, milestone 4's read line) are measured again with it.

## Where it does not reach yet

- The resident worker and viewer are Linux only for now (`view::resident_home`). On the iMac the
  block is the words side alone until resident mode is built there; doctor says which side ran.
- A Workers AI embedding is made for each prompt that asks (the iMac, once resident): it counts
  in the embedding budget and its daily cap, as search queries do, and doctor shows the count.

## Decision 44

The owner's answers, in their words, and the correction made at the owner's request are spec
decision 44: at prompt submit the hook asks the resident viewer (replacing decision 38's "Not now"
for it); imported history may appear in the per-prompt block (an exception to 4.5); cards,
summaries and imported history join claims, each above the threshold, and the block is empty when
nothing passes.

## Spec changes

With this note: decision 44, and pointers to it in 4.1, 4.5 and 4.6. With the code: 4.2 (the
prompt hook's steps), 4.6 (the two-sided threshold and `T`'s provisional calibration), the
`[inject] per_prompt_wait_ms` key, the parity table (row 31: per-prompt hints), and the settings
page's injection section (the wait, and which side answered last).

## Tests

- The hook asks the viewer only when it is up; the answer is gated and fenced; nothing below
  either threshold is shown; nothing already shown is shown again.
- A viewer that answers after the cap: the block is the words side's, built in time.
- No resident viewer (the iMac today): the words side, over cards, summaries and imports too.
- A prompt under 20 characters or an envelope asks nothing; imported records carry their label.
- The measurements above: answer times on both machines; `T` from the no-answer questions.
