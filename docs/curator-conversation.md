# The curator's windows as one conversation (decision 107)

What decision 107 asks for, what was measured before it, and the design to build. Nothing of it
is built yet: "As built", at the end, says what is.

## The decision

The curator's prompt cache is to work about as well as claude-mem's observer's, at about the
present bill. Two things do it together, as they do for claude-mem's observer:

1. A long tool output is sent short.
2. What an entry was sent before goes again with each call, as one growing conversation that the
   entry reads from its prompt cache.

Until now each record was sent once and only the instructions repeated
([prompt-cache.md](prompt-cache.md), "How far caching goes"): about 15% of the prompt tokens were
read from the cache, and that was the design's ceiling. Decision 107 gives that design up. Both
parts are built behind settings that are off, and neither becomes the default before the memory's
quality is measured once (below).

## What was measured (2026-11-13, this PC's stores, read-only)

**A curator prompt today.** Over 4,666 answered windows since 2026-11-10 06:21 UTC: the
instructions about 15% of the prompt tokens (read from the cache in 96 to 99% of the calls), the
window's new lines about 76%, the claims already known (candidates and carried) about 9%. The
known claims come after the lines, so no cache can serve them; only 2.3% of what is sent is a
known claim that the call just before also carried, so putting them first would gain about
USD 1 a month.

**The window text.** Over 4,701 answered windows, by `budget::estimate`: tool output 89.1%, tool
input 10.3%, prompts 0.5%, replies 0.1%. By whose records: Claude Code's subagents 77.6% (28,250
tool calls with an `agent_id`), its main sessions 12.5%, Codex 9.5%, Grok 0.3%. What a rule
leaves of the text:

| Rule (a tool's input, an output's head and tail, in characters) | Every tool call | Subagents' calls only |
|---|---|---|
| 300, 300 + 300 (`[summary] shrink`, as built) | 23.6% | 36.9% |
| 1,000, 1,000 + 1,000 | 50.9% | |
| 2,000, 1,500 + 1,500 | 63.2% | 69.5% |
| 2,000, 4,000 + 4,000 | 88.2% | |

**claude-mem's observer, the same days.** Its log gives each call's input, output and cost. On
2026-11-10 UTC: 3,802 calls, 117.6 M input tokens, 96.8% of them read from the cache (by the cost),
3.7 M new, USD 2.36, where the same calls uncached would cost USD 15.16. One observation adds 895
tokens at the median and its answer 246 on average; a conversation is retired on a budget (65 times
in that day's log). The model used below, "a call reads from the cache what the call before it in
its conversation sent", gives 94.0% and USD 2.74 for that day: 16% dear (20% and 25% on two other
days), so the estimates err on the dear side. That day oboete sent about 12.3 M new tokens in its
answered curator calls and 3.3 M in refused ones. claude-mem's log of that date shows 4,372
observations enqueued for the 7,051 tool calls its hooks logged (3,833 for 25,890 the day after, a
day of many subagents); oboete curated every tool call.

**The estimate.** Each paid entry's real curator calls since 2026-11-10 06:21 UTC, priced again
as the design below would send them: the instructions and the stretches before read at a tenth,
this window's stretch written at 1.25 times, the tail and the answer as now; a conversation
restarted when the call before started more than 270 s ago (1,740 s on OpenAI's API) or the
prompt would pass 90,000 tokens. Share of the prompt tokens read from the cache, and the bill
against today's:

| Window text | Messages API, one call a window | Messages API, a conversation | OpenAI's API, one call a window | OpenAI's API, a conversation |
|---|---|---|---|---|
| whole | 16%, 1.00 | 70%, 1.31 | 16%, 1.00 | 83%, 1.59 |
| 50.9% left | 26%, 0.70 | 82%, 0.94 | 24%, 0.65 | 90%, 1.20 |
| 36.9% left (subagents' calls short) | 31%, 0.61 | 85%, 0.83 | 28%, 0.56 | 92%, 1.08 |
| 23.6% left (every call short) | 38%, 0.53 | 89%, 0.73 | 34%, 0.46 | 93%, 0.94 |

Over ten days the Messages API's last two rows read 88% at 0.89 and 91% at 0.80. Almost all of this
PC's bill is the Messages API's (USD 19.09 of November's first 12 days' 20.56). A conversation
holds 10 to 17 windows at the median and 24 to 50 at the 90th percentile; restarted at 45,000
tokens it holds 5 to 9, for 74% at 0.75 and 80% at 0.65. With an hour's lifetime, written at twice
the input price, the same two rows read 88% at 1.04 and 91% at 0.88: two to three points for a
fifth of the bill, so five minutes stay.

The answer tokens are taken as unchanged, and so is the share of answers `curate::check` refuses
(7 to 22% of a day's calls to the Messages entry, #368): a prompt that raised it would cost what
this table saves.

## The design

Names are working names.

### The prompt

With `[summary] conversation = true` a window's prompt has three parts:

    [instructions][stretch][tail]

- **The instructions** are the same bytes for every window: they name no fence and no number of
  cards (a grown window's instructions are other bytes today, decision 72, and miss the cache for
  it). They say that one or more stretches follow, the oldest first; that only the last one is new
  and is the one to extract from; that the earlier ones were read before and are never quoted,
  never a card's subject.
- **A stretch** is one window's lines between its own fence lines, opened by the sentence that
  names its fence (`curate::FENCE_SENTENCE`, so the CLIs' split of decision 86 and
  `provider::instructions_end` work as they do). The fence is the hash of the stretch's text, as
  now: no text holds its own fence. A stretch holds nothing that can change between two sends of
  the same window.
- **The tail** is what changes with each call: the sentence that names the new stretch by its
  fence and its line ids, how many cards the window may give, and the claims already known between
  fence lines of their own (candidates, then carried, as now; they change while a window waits).

Line ids stay `L1`, `L2`, ... within a window, so `Window`, `locate`, `check`, the hold's request
hash and recuration are untouched; the tail says which stretch the ids are of. A claim that quotes
an earlier stretch finds no such quote in this window's line and is dropped, as an unanchored one
is now (#968 counts it). If the measurement shows more refused answers, ids that no two stretches
share (a window's first seq in them) are the next thing to try.

Off, `curate::prompt` is used as it is, byte for byte.

### The conversation

An entry that marks its own cache talks: the Messages API, and OpenAI's own API for its GPT-6
models. It is sent

    [instructions]*[stretch 1]...[stretch n-1][stretch n]*[tail n]

where `*` is a breakpoint, stretches 1 to n-1 are what this entry was sent in its calls before,
each a block of its own, and stretch n is this window. The call after it reads through stretch n
and writes stretch n+1: its breakpoint finds the entry one block back (the lookback is 20
blocks). Two of a request's four breakpoints are used. An entry whose `extra` marks a cache of its
own does not talk: ours would follow its marker, and the API refuses a longer cache after a
shorter one. Nor does one that declares no `max_request_tokens`: a conversation needs a size to
start again at. Every other entry (the CLIs, the OpenAI-compatible ones) gets
`[instructions][stretch n][tail n]`.

The state is per entry: its stretches, when its last call started, how many tokens its last
answer said that prompt was, and its key (below). The worker's curation phase owns it, in memory
only, for one run of the worker under its lock: a worker that left starts again, and by then the
entry's cache has lapsed. `oboete recurate` and every other role's calls are as now.

A window sent again (after an answer its check refused, or a hold, the same or grown by then)
takes its own earlier stretch's place: no window's lines are in a prompt twice, and the cache is
read through the stretch before it.

An answer joins nothing: the known claims already carry what was kept, and an answer sent again
would be written at 1.25 times.

### Restarting

A conversation restarts, as `[instructions]*[stretch]*[tail]`, when any of these holds:

- its key differs from this window's (the gate, below);
- the entry's last call started longer ago than its cache keeps an entry, less a margin: the
  Messages API measures five minutes from a request's start, and a read renews it;
- the prompt would pass the entry's `max_request_tokens` less a tenth, or what admission lets
  through with the answer's bound (`budget::admit_with_history`), whichever is less. What goes
  again is counted in the entry's own tokens: the last answer's `prompt_tokens`, less the
  estimates of the last tail and of a stretch this window's takes the place of; only this stretch
  and tail are counted from their estimate (all of it, when that answer reported no tokens). An
  estimate is taken for more tokens than the entry counted of late: the most its tokens were to
  its estimates over its last 50 answers, and a third over that, or 2.6 until it has answered
  five times. The budget counts with the median of those ratios, but a window is not the average
  one: over 20,137 answers of the Messages entry on this PC the median was 1.71, the 90th
  percentile 1.84, the 99th 1.92 and the most 2.55. A window that counts further over its
  estimate than that can still pass the ceiling, and pays the higher price once. Haiku 5.5's
  price is five times past 100,000 tokens, tokens read from the cache count toward it, and a
  request past it pays the higher price for all of its tokens;
- the entry's model or address changed, or its last call ended in an error, a timeout or a
  status: what its cache holds is then unknown.

After a call that was sent and answered, accepted by `check` or not, its stretch joins the
conversation. A restart only loses the cache.

**A cache that does not answer.** A conversation costs less than separate calls only while what
is sent again is read from the cache: a call that went on and read less than half of it wrote all
of it instead, at 1.25 times. Two such calls in a row, and the entry keeps no conversation for an
hour: its calls are the prompt with its one stretch, as before. What is sent again is taken at
its least for this (the last tail, and a stretch replaced, at the high ratio above), so a window
sent again is no miss for the stretch it replaced, however large. `provider_calls.earlier` (how
many stretches went before a call's own; NULL for a call that was no turn) beside `cached_tokens`
shows each such call.

### What a window was shown

A window in a conversation was shown the stretches before it. Its op says so, as a turn's op does:
`read`, the spans of those stretches. The one reading of what an op was made from
([provenance.md](provenance.md) P1; `Provenance` in src/raw.rs) takes a window op's span and its
goals as the records it was shown; it takes `read` too. So one forgotten or masked record takes the
summary, the cards and the claims of its own window and of every later window of that conversation
(10 to 17 windows at the median on this PC's calls), and they are curated again. That is the price
of a larger restart size, and a reason to lower it if forgets or masks come often.

Until a forget's purge has rewritten them, what is read is held back as a card of the forgotten
record's own window is: the stretches a window read are records its curator was shown, as its
goals are, so a removal from one that its op does not list hides its cards (docs/cards.md K4), and
the summaries of the turns that read those cards (docs/summaries.md T7: a turn op lists the
stretches with the windows' spans). The op lists what was already removed from the stretches when
its window was cut, with what was removed from its own records.

### The gate

Everything that leaves the machine passes the egress gate (spec 3.3, 5.5, 6.4; milestone 5 D1
rule 12), and a stretch sent again passes it again:

- The key is made of what the gate holds for a window: the rules' version, `Reading` (the
  exclusion list, the sessions it excludes, `fence_count`: what forget denied, a record or a
  claim, and the ranges a rescan masked; and how many tombstones raw.db held, a removal of any
  kind, whether or not the fence counts it), and the store's handle. A forget, a mask, any other
  removal, an exclusion or a rule change gives another key: nothing rendered before it is sent
  again. The language and the prompt's wording need no key: a conversation goes on only with the
  same instructions, byte for byte.
- The gate still runs before each call, as now (`dispatch`, `Reading::still`, `rules_still`): at
  a send the state is the one the window was read under, and every stretch sent again was
  rendered under an equal key. `Reading` is taken before a window is cut, and now holds the
  tombstones too: a call of any role goes out only while no record was removed since its inputs
  were read. Before, a tombstone stopped a call only by the denial or the mask written with it.
  The count needs no such pairing: whatever writes a tombstone stops the calls of what was read
  before it, and so nothing rendered before it goes again.
- What rewrites a record's text holds the worker's lock: a forget's purge, retention's expiry
  (`retention::rows`), a restore, a rebuild. The worker that holds it steps aside by ending its
  loop, and the conversation ends with it, so none of them runs within a conversation and
  retention needs no marker in the key (an earlier draft of this note gave it one). Within one
  run the stores can be closed and opened again (a damaged raw.db restored): every open of
  raw.db has an id of its own in the key, so the next window starts again.
- A mute changes which claims are shown, never a record's lines: it can change a tail, which is
  made for each call, and restarts nothing (the earlier draft listed it).
- Prompt injection (spec 3.3): each stretch's fence is the hash of its own text, the tail names
  the stretch to read after all recorded text, and the code gates do not change: a claim needs its
  quote in a line of this window, a decided one the developer's own line.

What changes for the person: a window's lines are sent to the same entry again with each later
call of its conversation, and that entry's cache holds them until five minutes (thirty at OpenAI)
after the last call. Without the setting they are sent once and not written to a cache. The text
that says what leaves the machine and [prompt-cache.md](prompt-cache.md) say so.

### The shorter window text

`[summary] shrink` takes `true` (every tool call, as built), `false`, or `"subagents"`: only the
tool calls a subagent made (an `agent_id` on the event). The rule is 3.5 (a)'s as built: a tool's
input up to 300 characters, an output over 600 as its first and last 300, the owner's-words tools
whole, the op listing what it shortened. A subagent's report comes back to the main session as
its tool's output, which `"subagents"` leaves whole.

## Measuring before a default changes

Spec 3.5 (a) stands, both of its clauses: the shrink is enabled only if the curator's input
shrinks by 30% or more while the recall of decisions, lessons and fixes drops by 0.02 or less. Its
one measurement (docs/milestone-3.md) passed the first clause for every tool call (70.9% less input
on the dev set) and recalled 22 decisions against 24 of 44, within two runs' spread and failing the
rule declared before it; lessons, fixes and cards were not measured, and `"subagents"` was not
built. The measurement of decision 107 is one run of the kind decision 34 allows, its cost shown
first, with the arms, the exact cost and the rule that fails an arm fixed before it:

- the first line is the share of answers `check` refuses, for an entry that talks and for one
  that does not (the new prompt reaches both): an arm that raises it fails, whatever it recalls;
- then the labelled lines of milestone 3 (decisions recalled, overturns still current), with
  enough runs of an arm to know its spread;
- then what no label covers: lessons, fixes and cards, judged on a sample of windows side by side;
- and each arm's input against the arm that shows every call whole: an arm whose input is not 30%
  less is not made the default, whatever it recalls.

The dev set's transcripts hold subagents' files (968 of 998), so `"subagents"` can be told from
`true` on it.

After it, on this PC: `provider_calls` against the estimate above.

## Order of the work

1. This note and decision 107.
2. `[summary] shrink = "subagents"`.
3. The three-part prompt behind `[summary] conversation`, with no conversation yet.
4. The conversation for the Messages API: the state, the restarts, the op's `read` and forget's
   rule over it, the tokens counted from answers. The text that says what leaves the machine
   (MUST-M23) and [prompt-cache.md](prompt-cache.md) change in the same PR: from the build that
   lets a person turn the conversation on, they say that a window's lines are sent again and held
   in the entry's cache. Security review.
5. The same for OpenAI's own API: its explicit breakpoints are checked with a live pair of calls
   first (whether a later breakpoint reads what an earlier request wrote at an earlier one).
6. The measurement, then the defaults, the settings page and its text, and the parity table's
   row.

## As built

- **`[summary] shrink = "subagents"`** (#972). A window shows short only the tool calls a subagent
  made, by the rule `true` uses for every call: a tool's input up to 300 characters, an output over
  600 as its first and last 300, the owner's-words tools' output whole, the op listing what it
  showed short. An event names a subagent by a non-empty `agent_id`, as capture stores it from a
  hook and from a transcript. `true`, `false` and no line do what they did; any other value is a
  configuration error. Off is the default.
- **The three-part prompt** (#973). With `[summary] conversation = true` a window's prompt is its
  instructions, its stretch and its tail, as "The prompt" above says. The instructions name no
  fence and no number of cards, and are the same bytes for every window. The stretch is the
  window's lines between fence lines made of the hash of those lines, opened by the fence
  sentence. The tail names the stretch by its fence and its line ids, says how many cards the
  window may give, and holds the claims already known between fence lines of their own
  (`=== KNOWN <16 hex> ===`). The Messages API's breakpoint, OpenAI's and the CLIs' system text
  split where they did, at the fence sentence. Every entry gets one stretch: only the
  instructions are read from a cache, as before, and a grown window's are now any other
  window's. Off is the default, and off a prompt is the bytes it was.
- **What reads `read`** (#974). No window op lists `read` yet; what will read it is built first.
  The one reading of an op's provenance takes a window op's `read` as the spans it read. Forget's
  rule for what a window showed (rule a) takes the records of those spans with the window's own
  and its goals, so the op, its cards and the claims of its batch go with a forgotten or masked
  record of a stretch it read, and the op keeps `read` when a purge takes its text. A card is not
  shown once a removal its op does not list lands in a stretch its window read (K4), and a turn's
  summary lists those stretches among what it read (T7).
- **The conversation, for the Messages API** (#975). With `[summary] conversation = true`, an
  entry of the Messages API that declares `limits.max_request_tokens` and marks no cache of its
  own is sent, before each window's stretch, the stretches of the windows before it in its
  conversation, each a text block, with breakpoints on the instructions and on the newest
  stretch (`provider::Talk`, kept by the worker's curation phase while it runs). It starts again
  as "Restarting" and "The gate" say, and keeps none for an hour after two calls in a row that
  read less than half of what they were sent again. The window's op lists `read`, and what was
  already removed from what it read; `provider_calls.earlier` counts the stretches before each
  call's own. A live triple of synthetic text on 2026-11-13 (claude-haiku-5-5): the second call
  read the 3,691 tokens the first wrote and wrote 1,923, and the third read 5,614 and wrote
  1,929. Every other entry, `oboete recurate`, and the summary's, My Work's and the judge's calls
  are as they were, and off is the default. One thing changes with the setting off too: the gate
  before a call of any role (`Reading::still`) now stops on any tombstone written since its
  inputs were read, where it stopped on a denial or a masked range ("The gate").

Not built yet: the same for OpenAI's own API, the measurement, the defaults and the settings page.
