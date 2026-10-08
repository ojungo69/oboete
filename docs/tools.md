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
steps and, since #403, notes) are indexed as claims are (FTS5 trigram, Unicode case
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

`timeline` around an anchor (any id `get` takes) or the newest, in claude-mem's shape: by day
(`### <date>`), a summary as `**S<op seq>** <request> (<time>)`, a prompt as its key, time and first
100 characters, the cards as rows of `| ID | Time | T | Title |` grouped by their first modified
file, the anchor marked, claims in their place by time. Designed and built after slice 1.

## Slice 3: vectors

The cards' and summaries' vectors, made by the embedding phase as claims' are, so search by meaning
reaches them (spec 4.10). After slice 2.

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
