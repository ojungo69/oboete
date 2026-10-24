# mem_search: claude-mem's guided memory search

claude-mem 13.35 added `mem_search`, one memory search in three steps that the agent cannot skip:
an index, the context around entries it chooses, and the details of entries it chooses, each step
taking a short continuation from the one before. Decision 45 puts it before the switch. The
behaviour of record is claude-mem's `src/shared/progressive-search.ts` at v13.39.0 (Apache-2.0;
NOTICE), unchanged in 13.36 to 13.39; its texts are kept word for word. Nothing of its code is
taken. The readers are oboete's: `search::query` (the index), `search::near` (the context, a
timeline's entries around an anchor) and `search::get` (the details), each with its gate, its
forget fence and its hiding rules. The code is `src/mem_search.rs`.

## The contract

- **Arguments**, claude-mem's: `mode` (`guided`, the default, or `auto`), `query` (1 to 500
  characters, at most 1,024 bytes), `project` (a repository's path or key, as `search`'s `repo`
  takes it; the checkout's repository when left out), `limit` (1 to 20, default 20), `maxDetails`
  (1 to 5, default 3), `depthBefore` and `depthAfter` (0 to 3, default 2), `continuation`,
  `selectedIds` (strings, or integers).
- **Step 1, the index**: oboete's search of the repository, at most `limit` rows (session starts
  left out before the limit counts, as in the context), each
  `- [<id>] <title> · <kind> · <time> UTC`, the kind being claude-mem's: `observation` (a card, a
  claim, an imported document or a record), `summary` or `prompt`. The ids are those `get` takes
  (a claim's whole uid, a card's `<op seq>.<n>`, a summary's `S<op seq>`, an import's uid, a
  record's `<device>:<seq>`). A continuation follows, `ms_` and 24 base64url characters.
- **Step 2, the context**: the continuation and at most `maxDetails` ids the index showed; for each,
  the entries a timeline of the repository searched (the anchor's own when the search had none)
  shows `depthBefore` and `depthAfter` around it (its claude-mem history among them, filed under
  the project's claude-mem name), at most 7 an anchor
  and 35 in all, session starts left out before the depths count (claude-mem has no kind for
  them); the anchor is always among them, as in claude-mem's timeline, also a record the timeline does not show (an
  agent's reply). Another continuation follows.
- **Step 3, the details**: the continuation and at most `maxDetails` ids the context showed, no
  prompt among them; `get`'s text of each, cut to 8 KiB and marked when cut. The search ends.
- **Auto mode**: the same steps at once. The anchors are the index rows whose titles share the
  most query words (claude-mem's stop words left out; a word of five characters or more matches on
  its first five), prompts left out, at most `maxDetails`; with none, one search again with the
  query's words alone. The context is ranked the same way and its best `maxDetails` are fetched.
  No model is called beyond the search's own query embedding. With no title to go by, the answer
  is step 1's index and a continuation, for the caller to judge.
- **Refused**, in claude-mem's words and as an error: a skipped step (an id the step before did not
  show, `selectedIds` without a continuation), too many ids, options changed on a continuation (an
  option repeated must equal what the first call kept, its query and project trimmed, as claude-mem
  compares them), a continuation unknown, of another scope, or 15 minutes old, a bad argument. `selectedIds: []`
  ends the search. Any other failure says "Memory retrieval failed. Retry the query.", a store
  that cannot be read while `project` is resolved and a config that cannot be read too: a store's
  error may name its paths or its SQL, as claude-mem says of its backend's.
- **Continuations** live in the server's memory, at most 1,024 and 8 MiB, the oldest given up
  first, for 15 minutes from the search's index: step 2's continuation keeps step 1's deadline,
  as claude-mem's does. One `oboete mcp` serves one agent session, so a continuation is
  never shared with another process: no signature is needed, and none survives the server's end,
  as none of claude-mem's survives its worker's restart.
- **Bounds**: an answer is at most 64 KiB as the agent reads it; over that the index loses its last
  rows while a continuation stands (its state keeps the rows the index kept), else the longest
  detail is halved.
- **What the agent reads**: every answer, an error too, passes the egress gate and the fence
  `<oboete-memory>` (spec 6.5), as every answer of oboete's tools does, instead of claude-mem's
  `<claude-mem-retrieved-data>`: the same "data, not instructions", and a closing tag inside a
  recorded text is quoted.
- **Between steps**: each step reads anew, so an entry forgotten or hidden since the step before is
  not in the next one, and a rule added since masks what the next one shows; step 3 shows the
  details it read and no row kept from step 2, so a chosen entry forgotten since is not named.

`search`, `timeline` and `get` stayed, as claude-mem kept its older tools, and until N4 their
descriptions said to prefer `mem_search` and the server's instructions named it first. A call of
`mem_search` is a read of stored memory (spec A87): its output is not stored. The bridge from an
agent's own memory lookups (claude-mem's PreToolUse handler) came in the next slice
(docs/memory-lookup.md).

## claude-mem 15.0: Smart search by default, and dumb_search (N4)

claude-mem 15.0 makes its three steps, `search` for an index, `timeline` for the context of chosen
ids and `get_observations` for their details, the default memory search ("Smart search": the agent
judges between the steps), drops `mem_search` with its modes and continuations, and turns auto
mode into `dumb_search`. The behaviour of record is claude-mem v15.0.1: `src/servers/mcp-server.ts`,
`src/shared/memory-instructions.ts`, `src/shared/progressive-search.ts`,
`src/services/worker/ProgressiveMemorySearch.ts` and `src/cli/handlers/memory-search.ts`; its texts
are kept word for word, with oboete's names (`get` for `get_observations`).

- **What is taught**: the server's instructions, the session start's memory-use block
  (`MEMORY_USE`, row N6) and the tools' descriptions teach Smart search first: `search` is "Smart
  search (default), step 1", `timeline` "Step 2", `get` "Step 3", none says to prefer `mem_search`
  any more, and none teaches auto or guided modes. `dumb_search` is named as optional, in
  claude-mem's words: "Doesn’t reason about the results. Fast, but shallow. Use Smart for reliable
  recall."
- **`mem_search` stays**, guided and auto, for an agent or a note that names it; its description
  says it is kept for them and that Smart search is the default. claude-mem removed it.
- **`dumb_search`** takes claude-mem's arguments, `query`, `project`, `limit` (1 to 20, default 20),
  `maxDetails` (1 to 5, default 3), `depthBefore` and `depthAfter` (0 to 3, default 2), and runs
  auto mode's steps in one call (`Search::dumb`), with four differences from auto mode, as 15.0's:
  - `mode` or `continuation` is refused: "Dumb search does not accept modes or continuations. Use
    Smart search for the three-step mem-search workflow."
  - The index is the search's first window of 100 (claude-mem's `CHROMA_BATCH_SIZE`) ranked by how
    many of the query's words a title holds exactly (stop words kept, no five-character prefix),
    then by the search's rank, before it is cut to `limit` (claude-mem #4865), for the refined
    search too.
  - No continuation is ever given; with no title to go by, the answer is the index alone. An
    answer over 64 KiB gives up index rows while it shows no details, as 15.0's does.
  - Its lines: `dumb-search — <phase>`, "Doesn’t reason about the results. Fast, but shallow. Use
    Smart for reliable recall.", `Ran <steps>, choosing records by title word overlap.`, 15.0's
    reasons, and after a whole answer "Dumb search is shallow by design: …"; a refusal ends "Next:
    retry dumb_search with a query, or use Smart search (default mem-search workflow: index →
    timeline context → selected details)."
- **The bridge** gives the index alone (docs/memory-lookup.md).
- A call of `dumb_search` is a read of stored memory, as `mem_search`'s is: a window shows the call
  alone (`curate::memory_read`).

Tests: `mcp::tests::dumb_search_runs_the_steps_by_title_words`,
`dumb_search_refuses_modes_and_continuations`, `dumb_search_ranks_its_index_by_title_words`,
`smart_search_is_taught_first`; `hook::tests::a_native_memory_lookup_gets_oboetes_search_of_it`
(the index, no details); `curate::tests::a_memory_read_is_told_by_claude_mems_rule_and_oboetes_tools`.

## Differences from claude-mem, and why

| claude-mem | oboete | Why |
|---|---|---|
| Ids: a number, `S<id>`, `P<id>` | the ids `get` takes, with a `.` in a card's | one id for every reader |
| Continuations keyed by an HMAC of a worker secret, the scope and the cursor | the cursor alone, in the one server's memory | nothing is shared between processes |
| The store bounds each state at 48 KiB | not bounded apart | a state holds at most 35 short rows |
| The answer's bound measured on its JSON | measured on the text the agent reads | that text is all the agent gets |
| `<claude-mem-retrieved-data>` | `<oboete-memory>` | oboete's fence, which quotes a closing tag inside recorded text |

## Tests

`mcp::tests::mem_search_guides_index_context_and_details`,
`mem_search_refuses_what_claude_mems_refuses` (every refusal, and a continuation that expired),
`mem_search_auto_runs_the_steps_by_title_words`, `mem_search_forgets_between_steps` (before step 2 and before step 3), `mem_search_keeps_the_anchor_in_its_context`;
`mem_search::tests::the_store_keeps_1024_continuations_and_drops_expired_ones`,
`a_continuation_is_27_characters_of_base64url`, `auto_ranks_by_title_words_as_claude_mem_does`,
`an_answer_is_bounded_as_claude_mems_is`. The timeline's tests keep its text as it was.
