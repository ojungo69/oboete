# The agent's tools: claude-mem's three layers on oboete's search

Unit C6 of the claude-mem parity plan (docs/cards.md, "Why"): the MCP tools an agent recalls memory
with, `search`, `timeline` and `get`, find and show the cards (docs/cards.md) and the session
summaries (docs/summaries.md) as claude-mem's `search`, `timeline` and `get_observations` find and
show its observations and summaries, beside oboete's claims. The behaviour of record is claude-mem
13.28.0's MCP server (`src/servers/mcp-server.ts` at 039c6160), described for an independent
implementation in the takeover notes' `claude-mem-spec/mcp-tools.md`. Nothing of its code is taken.

## Why

Owner decision 37 put three of claude-mem's search features before the switch: a kind filter, date
order and several ids in one call (claude-mem-parity.md row 6). And the cards and the session
summaries are what claude-mem's agent finds first; oboete's tools find claims, imported history
and records, not the cards and summaries the curator now writes (row 17).

## Slice 1: search and get

Q1. **The three layers.** The server's instructions and the tools' descriptions teach claude-mem's
workflow: `search` for an index of ids, `timeline` for what is around one, `get` for the full text
of the ids chosen, several in one call. The names stay oboete's: one `get` takes every kind of id.

Q2. **What `search` finds, in this order:**

1. current claims, as now (the owner's decisions and preferences first; oboete's merit);
2. cards, session summaries and imported documents, as one list: each kind ranked by its own full
   text (and its vectors, where the kind has them), the lists merged by reciprocal rank (`rrf`, as
   claims' full-text and vector lists are);
3. the records below them, prompts among them (`RawArm::Below`, as now);
4. unless `history`, the superseded, retracted and done claims (MUST-M11, as now).

`since`, `until` and the repository apply to every kind before it takes its part (MUST-M12).

Item 2 moves today's imported documents from a list of their own, after the claims, into one list
with the cards and summaries: claude-mem's imported observations and the cards are the same kind of
item, written before the switch and after it, and two lists would rank every one of one kind above
every one of the other whatever the words. The one evaluation after completion measures this order
against today's (owner decisions 34 and 35).

Q3. **Each item is read through its table's one reader** (K7): a card through `cards::read` (K4's
hide rule, K6's gate), a summary through `turns::read_row` (T7, T8; a skip is never a hit), a claim,
an imported document and a record as now. A hidden item is no hit and takes no place. Every line is
gated as every hit is, and the answer is fenced (spec 6.5).

Q4. **The full-text index of cards and summaries.** What a card shows (title, subtitle, narrative,
facts, concepts, files) and what a summary shows (request, investigated, learned, completed, next
steps and, since #403, notes; an index built before is built again once, knowledge.db's
`user_version` 1 marking it) are indexed as claims are (FTS5 trigram, Unicode case
folding) and searched with the same query terms (`search::terms`, its short-word rule included).
The rows are written in the transaction that
writes the card or summary, replaced and removed with it, and rebuilt with it (`rebuild`). The text
indexed is the stored text; the hit is read, and so hidden and gated, through Q3. Vectors of cards
and summaries are slice 3.

Q5. **The kind filter: `type`.** A comma-separated list, matched against each hit's kind:
`observations` (cards and imported observations), `sessions` (session summaries and imported
summaries), `prompts` (prompt records and imported prompts), `claims`, a card type (`bugfix`,
`feature` … `TYPES`), or a claim kind (`decision`, `preference` …). An unknown word is an error that
names the known ones. No `type`: every kind, as now.

Q6. **Order: `orderBy`.** `relevance` (the default: Q2's order), `date_desc` or `date_asc`: the
same hits, by their own time (a card's window time, a summary's turn end, a claim's validity, a
record's time), ties by Q2's order. The limit applies after the order.

Q7. **One line per hit, ranked.** As now: the id `get` takes, the UTC time, the kind and standing,
the repository when every one was searched, the title and a snippet; and, on a card's line and an
imported observation's, claude-mem's read cost: `~N` tokens of what `get` would return (its
characters / 4, rounded up), as claude-mem shows it on observations only. A card's id is its
`<op seq>.<n>` (`<device>.<op seq>.<n>` for another device's), a summary's `S<op seq>`
(`S<device>.<op seq>`), as session start shows them.

Q8. **`get` takes several ids.** `ids`, a list of 1 to 20 ids as `search`, `timeline` and session
start give them, or `id` for one; one of the two. The answers come in the order asked, each under its
id; an id of nothing (or of an item hidden since) says so in its place, and the others are still
answered. A card in full is its type, title, subtitle, facts, narrative, concepts, files, repository,
agent and time; a summary its request and four sections. Each through Q3.

Q9. **The CLI says the same.** `oboete search` takes `--type` and `--order`, and `oboete get` takes
several ids, with the MCP tools' answers.

## Slice 2: timeline

The behaviour of record is claude-mem 13.35.0's `timeline` tool (`src/servers/mcp-server.ts`) and
its worker's `SearchManager.timeline` and `renderTimeline` (`src/services/worker/SearchManager.ts`,
`src/shared/timeline-formatting.ts`, at fa8ab09). Built after #439's read fence (#446).

T1. **Arguments.** `anchor`, any id `get` takes (a claim's, a card's `<op seq>.<n>`, a summary's
`S<op seq>`, an imported document's, a record's `<device>:<seq>`), or `query`, whose anchor is the
first hit `search` gives for it in Q2's order (of its ten, so a pair it keeps together is not
dropped for want of a second place), with the same repository; not both (an error naming
the two). A query with no hit says so. Neither: the newest entries, `limit` of them (default 20, at
most 100), as now. With an anchor or a query, `depth_before` and `depth_after` (default 3 each, as
claude-mem's tool describes them; at most 50): how many entries before and after the anchor;
`limit` is not used then.

T2. **What it lists.** Today's entries (current claims, imported documents, session starts) and
claude-mem's: current cards (`cards::read`: K3, K4, K6), session summaries that were not skipped
(`turns::read_row`: T7, T8) and prompts (prompt records, read and gated as search's record leg reads
them). Each through its table's one reader (Q3): a hidden item is no entry and takes no place. The
repository, `all` and the read fence as now.

T3. **Order and depth.** Every entry by its time (a card's window time, a summary's turn end, a
claim's validity, a record's time); entries of one time a prompt first, then claims, imported
documents and session starts, then cards, then summaries (as the session start shows a summary after
the cards of its time), then by their stored order (card 10 of a window after card 9). The anchor's entry, then the `depth_before` entries
before it and the `depth_after` after it; a side with fewer has fewer (claude-mem's
`filterByDepth`). An anchor that is a record of another kind than a prompt has no entry of its own:
the entries around its time, those of its own time before it. So has an anchor of another
repository than the one asked (claude-mem's anchor is not filtered by project either). An anchor whose item is hidden or gone is an error, as `get` reports it.

T4. **The answer**, claude-mem's text, with oboete's ids:
- `# Timeline around anchor: <id>`, or `# Timeline for query: "<query>"` and then
  `**Anchor:** <id> - <title>`; `# Timeline: the newest <k>` without either; then
  `**Window:** <n> records before -> <m> records after | **Items:** <k>` (without an anchor, the
  window line is left out);
- by day in the local time zone, oldest first: `### <Mon D, YYYY>`;
- a summary: `**🎯 S<op seq>** <request, else Session summary> (<Mon D, h:mm AM>)`;
- a prompt: `**💬 Prompt <device>:<seq>** (<Mon D, h:mm AM>)`, then `> ` and its first 100
  characters, `...` after a longer one;
- a claim, an imported document and a session start: `**<id>** <kind and standing> <one line>
  (<Mon D, h:mm AM>)`, the claim's id its first 12 characters, as search's lines show them;
- cards grouped by their first modified file, else their first read file, else `General`, as paths
  from the working directory where they are under it: `**<file>**`, then `| ID | Time | T | Title |
  Tokens |` and its rule, a row `| <id> | <h:mm AM, or " for the minute before> | <icon> | <title,
  else Untitled> | ~<N> |` (the session start's icon of the card's type, Q7's `~N`); a new table when
  the file changes, and after any other entry;
- the anchor's line ends ` <- **ANCHOR**` (on a card's row, after its title);
- with every repository (`all`), each line and each table's file ends with ` [<repository>]`, as
  the list did; a `|` in a card's title is written `\|`, so it stays in its cell.
Every line gated, the answer fenced (spec 6.5), as every tool's.

T5. **Where.** The MCP tool and `oboete timeline` (`--anchor`, `--query`, `--before`, `--after`,
`--limit`) give this text. The viewer's pages keep their JSON list of today's entries (its own
slice), and `search::b::timeline` stays as it is for them.

## Slice 3: vectors

The cards' and summaries' vectors, made by the embedding phase as claims' are, so search by meaning
reaches them (spec 4.10). After slice 2. It is also what per-prompt delivery by meaning stands on
(decision 40, G23 of the 13.34.2 comparison).

V1. **Two more kinds of document.** A current card (`o`, keyed `<device>.<op seq>.<n>`) and a
session summary that was not skipped (`s`, keyed `S<device>.<op seq>`) are embedded as claims and
imported documents are, in the same phase and the same calls: claims first, then cards, summaries,
imported documents and records, as search lists them (Q2). The text is what their full-text index
holds (Q4), made by the one function that writes that index, so the words full text finds and the
vector come from the same text; it is gated, each value alone as its reader gates it (K6), and cut
as every document's is (D8). Its vector is cached by that text and where each value lies in it, so
values that join to one text at other places, which gate differently, share no vector.

V2. **Passed over.** A card or summary of an excluded repository, or made from a record of a
session that touched one, is not sent, as a claim that quotes such a record is not (D13). The
records are the ones its hide rule reads: a card's window and goals (K4); a summary's own session,
and the windows whose cards it read with their goals (T7). It gets an `excluded` mark, judged again
when the exclusion changes. A window's records that curation kept back, of a session the list named
then, count too: the row keeps the range, not what was kept back, so such a card waits for the
undo with them. An empty text is marked `empty`, and so is a card or summary a removal hides (K4,
T7) before it has a vector, as a removed record is: no reader shows it, so it is sent nowhere. One
hidden after it has its vector keeps it, as it keeps its full-text row: its hit is read through its
reader, which hides it (Q3). A text the redaction rules leave blank, of any kind, is marked with
those rules' version, not `empty`: when the rules change it is read again, and sent if the new
ones leave it words, as an `excluded` mark is judged again (doctor counts it with `empty`).

V3. **The vector follows its row.** The consumers that write the cards and the summaries call
`embed_phase::touched` for each row they replace or remove, in the transaction that does it, as
the claims' consumer does: a recuration that replaces a window's cards (K3), a rewind and a
rebuild. A row whose text is gone or changed loses its vector; one whose text is the same keeps it,
under its repository and time now. A rebuild sends nothing again: vectors are kept by their text's
hash. Milestone 5's purge of a card or summary goes through the same call (its plan lists every
generation's vectors).

V4. **Search.** The cards' list and the summaries' list in Q2 item 2 are each their full-text list
fused with their vector list by `rrf`, as the claims' lists are. `type`, the repository, `since`,
`until` and the session `search` skips apply inside the vector search, as for imported documents and
records, and a hit is still read through its table's one reader (Q3). The vector list is read from
the bounded candidate pool, nearest first, and its depth counts only the rows the reader shows: a
hidden row takes no place in it. The pool is the 400 candidates the bit index gives (D8), as for
the other legs, so a shown row is missed by its vector only behind 400 hidden ones nearer to the
query (full text still finds it), until milestone 5's purge takes a removed row's vector with it.

V5. **Cost.** A card or summary is one more document in the phase's batches, under the embedder's
caps (Workers AI's daily requests and monthly USD; the local model is free). Cards come about one
per 4.6 tool uses (decision 39), and a summary one per reply.

V6. **Doctor.** The documents waiting for a vector, in `oboete doctor` and on the page's doctor
list, count the cards and the summaries beside the claims, imported documents and records.

## Tests of slice 3 (through the embedding phase's stub answers and `search`)

1. A card and a summary get vectors after the claims; a search whose words no full-text list holds
   finds each by its vector, and `type` keeps only the kind asked.
2. A recuration's replaced cards lose their vectors and its new cards get theirs; a rewind removes
   them; a rebuild maps the same vectors and sends nothing.
3. A skipped summary gets no vector, and an excluded repository's card and summary are not sent,
   nor those made from a record of a session that touched it: a window across sessions, a goal, a
   window a summary read.
4. A card a removal hides is not found by its vector, nor takes a place in the vector list (100
   hidden cards nearer than the one shown); a card and a summary hidden before they had one are
   not sent.
5. A rule on a whole value of a card or summary, across its lines, holds in the text sent.
6. Cards held for an answer are each sent again alone.
7. A card or summary whose values join to another's text at other places gets its own vector, its
   own gated text sent.
8. A card and a summary the rules leave blank are sent once the rule goes; doctor counts them as
   `empty` until then.

## Slice 4: save_memory

claude-mem 13.35's `save_memory` (`src/servers/mcp-server.ts` and `src/services/worker/save-memory.ts`
at v13.39.0), before the switch by decision 45. The agent saves a durable note, decision,
correction or handoff on purpose; search finds it, and the session start never shows it.

- **Arguments**: `text` (required), `title`, `project`, claude-mem's names. claude-mem's
  `metadata` is not read (nothing in oboete reads provenance); a call that sends it is not refused.
- **Where it goes**: the repository of the checkout the server runs in, or the one `project`
  names: a directory (its own `.oboete.toml` and the folders excluded around it are read too), or
  a repository key `search` knows (`host/owner/name`, or an imported `claude-mem:<project>`). A
  bare claude-mem project name is refused: a note filed under it would be found from no
  checkout.
- **What is stored**: one document of source `oboete-note`, kind `note`, in raw.db's op log
  (`Raw::append_note`), which the imported-documents consumer indexes as it indexes claude-mem's
  history: no new op kind, so no change of the store's format. Title and text pass the capture
  gate (`capture::note`): secrets masked (unless the switch is off, decision 43) and private
  blocks taken out. A title left out is the text's first 60 characters on one line, and `...`
  when there are more, as claude-mem's.
- **Refused**: a text with nothing outside private blocks; a text or title over 64 KiB, or a
  note whose op would be (claude-mem has no limit; oboete embeds what it keeps, and refusing says
  so where clipping would not); a note where nothing is recorded, by the writer's folder or the
  note's repository, also in the write's transaction, as a work state write is (slice 4a); a
  note forget names.
- **The same note again**: its uid is `oboete-note:` and 16 hex digits of a hash of its
  repository, title and text, so a repeat answers the same uid and appends nothing once the
  first is indexed and its op is still in the op log (claude-mem's exact-repeat hash is its
  session, title and text). The op log decides, not the index: after a restore lost the op and
  before the worker rewound the index, the note is appended again.
- **The answer**: `Saved memory <uid>: <title>. Repository: <repository>.`, once the worker has
  indexed it (at most 10 seconds, as `oboete pref add` waits), else the same with "The worker
  indexes it when it next runs". claude-mem answers `Saved memory #<id>: <title>. Project:
  <project>.`
- **Found**: `search` labels it `note` (claude-mem files it as an observation of type
  `discovery`), `get` reads it whole "(saved by an agent)", `timeline` lists it with imported
  documents. The session start never shows it (claude-mem's needs a concept, which a saved note
  has none of); the per-prompt block takes it with imported history's slice (#506).
- **The call itself**: capture keeps a `save_memory` call, bare or of oboete's or claude-mem's
  server (claude-mem's `isExplicitMemoryWrite`), without its input and output, and the curator
  sees its name alone: the note is not made a card or a claim a second time.
- **Not a claim**: an agent wrote it, so it is never delivered as a decision; it is data inside
  the fence wherever it is shown.

Tests: `mcp::tests::a_saved_note_is_found_by_search_and_get_and_kept_once`,
`a_saved_note_passes_the_gate_and_an_empty_or_huge_one_is_refused`,
`a_note_goes_where_project_says_and_nowhere_never_recorded_or_forgotten`;
`curate::tests::a_note_saved_to_memory_is_not_shown_to_the_curator`;
`capture::tests::a_memory_read_is_stored_without_its_output` (a saved note's call too);
`hook::tests::a_failed_memory_read_is_no_failure_of_the_work` (a failed save too).

## Differences from claude-mem, and why

| claude-mem | oboete | Why |
|---|---|---|
| `get_observations(ids)` | `get(ids)` | one `get` for every kind of id oboete has (claims, cards, summaries, imported documents, records) |
| Search output: tables grouped by day and file, in date or first-appearance order | one ranked line per hit | relevance order is what oboete's search was measured on (docs/milestone-1.md); `orderBy` gives the dates |
| Observations, sessions and prompts fetched separately, then shown together | claims first, then cards, summaries and imported documents fused by rank, then records | the owner's decisions come first (oboete's merit); one ranked list instead of three |
| `offset` paging | none (`limit` up to 100) | claude-mem's semantic path ignores it (claude-mem-parity.md row 6) |
| `get_tool_uses` (raw tool input and output) | `get` of a record's key | a record is the raw event, gated |
| Advertised default limit 20, actual 50 / 50 / 20 per kind | default 10, at most 100 | as now |
| Code-structure tools, corpora | none | left out by decision 37 |
| `timeline`: anchor or query required; an observation id, `S<id>`, or a time | anchor or query, else the newest entries; any id `get` takes | oboete's newest list stays (the viewer's and the CLI's habit); a record anchors by its time |
| `timeline`'s depths: described as 3, 10 when left out | 3 when left out | the tool's description is what an agent reads, and three entries a side cost less context |
| Timeline token estimate: an observation's narrative / 4 | Q7's `~N`: what `get` returns / 4 | one estimate across search and timeline |
| Timeline lists observations, summaries and prompts | also claims, imported documents and session starts | oboete's claims are what it knows best (owner decision 37's superset) |

## Tests of slice 2 (through the MCP tool and the CLI)

1. Around a card: the cards, the summary and the prompt of its window and the claims of that time,
   by day and by file, the anchor marked, the depths kept (fewer on a short side).
2. `query`: the anchor is search's first hit, named in the header; `anchor` and `query` together is
   an error, and a query with no hit says so.
3. A hidden card (K4), a skipped summary and a forgotten claim are no entries and take no place; a
   rule added after they were written masks their lines.
4. Without an anchor: the newest entries, as now; the viewer's list is unchanged.
5. A record anchor that is no prompt gives the entries around its time, none marked; an anchor of
   nothing is an error.

## Tests of slice 1 (through the MCP tools and the CLI)

1. A card and a summary that hold the query's words are found, below a claim that holds them and
   above a record that holds them; their ids are the ones `get` takes.
2. A card hidden by a removal, a summary hidden by a removal or skipped, are not found; a rule added
   after they were written masks their lines.
3. `type` keeps only the kinds named (each of Q5's words), and an unknown word is an error naming the
   known ones.
4. `orderBy` `date_desc` and `date_asc` order the same hits by their own time.
5. `get` with three ids, one of nothing, answers the two in the order asked and says the third is
   not found; more than 20 ids is an error.
6. The index follows the rows: a recuration that replaces a window's cards finds the new ones and not
   the old; `rebuild` gives the same hits.
7. The CLI's `--type`, `--order` and several ids give the MCP tools' answers.
