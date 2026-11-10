# The prompt cache

What each AI entry reads from its prompt cache, why it read so little before decision 86, what
changed, and how far caching can go while oboete sends each record once.

## Before (2026-11-03 00:00 to 2026-11-10 00:00 UTC, this PC's providers.db, answered calls)

| Entry, role | Calls | Tokens read from the cache in a call | Share of the prompt tokens |
|---|---|---|---|
| anthropic (Messages API), curator | 5,596 | 2,618 in 95.4% of the calls, else none | 20.7% |
| anthropic, summary | 535 | 759 in 59.1% of the calls, else none | 13.9% |
| anthropic, My Work | 255 | 1,124 or 1,101 in 12.9% of the calls, else none | 3.3% |
| claude (CLI), curator | 3,739 | 714 in 99.4% of the calls (a repeated prompt's whole in 4) | 6.6% |
| claude, summary | 209 | none | 0% |
| claude, My Work | 147 | none, but a repeated prompt's whole in 4 calls | 4.6% |
| openai (OpenAI's own API, gpt-6-luna), curator | 517 | none | 0% |
| openai, summary and My Work | 68 | none | 0% |
| codex (CLI), curator | 9 (403 more refused by the isolation gate) | none | 0% |

Why:

- **The CLIs.** claude caches its system prompt, which held only `CURATOR_SYSTEM` and the schema
  (the 714 tokens). The curator's instructions, the same bytes for every window of a size, went in
  the turn with the record, so they were never read back. codex was built the same way.
- **OpenAI.** Decision 80 set `prompt_cache_options.mode` to `explicit` so that GPT-6 would not
  write each whole prompt at 1.25 times the input price, but set no breakpoint: with none, OpenAI
  caches nothing and writes nothing.
- **The Messages API.** The fenced prompt's instructions have their own block with a breakpoint
  (G09), and the 2,618 tokens are those instructions with the schema the API adds. A summary or a
  My Work job often comes more than five minutes after the last one of its kind, when the entry has
  already expired.

## Decision 86 (2026-11-10)

1. **The CLIs** get a fenced prompt's instructions, everything before `curate::FENCE_SENTENCE`, in
   their own system prompt: claude's `--system-prompt-file` (after `CURATOR_SYSTEM` or
   `MY_WORK_SYSTEM` and the schema) and codex's `model_instructions_file`. The turn on stdin starts
   at that sentence; its `@` are read as before. When codex's file cannot be named (a path that is
   not UTF-8), codex keeps its own instructions and the turn the whole prompt. An unfenced prompt
   goes as before. Live on 2026-11-10, claude's curator read 2,177 tokens from its cache in a call
   (714 before), and its summary up to 981 (none before). codex's call was refused by the isolation
   gate before anything was sent (a codex feature oboete has not reviewed is on), as on this PC
   all week: its part is checked by the unit tests only.
2. **OpenAI's own API**, for its GPT-6 models (`config::gpt6`), gets a fenced prompt as two text
   parts of the user message: the instructions, with `prompt_cache_breakpoint: {"mode":
   "explicit"}`, then the rest, which is neither cached nor written; an entry's own
   `prompt_cache_options` in `extra` still wins, and can cache the record too. A prefix must be at
   least 1,024 tokens: the curator's instructions are 1,074 (`o200k_base`, 2026-11-10), and with
   the schema and the message's framing before the breakpoint a live call on 2026-11-10 read 1,402.
   They are written at 1.25 times, kept 30 minutes and read at a tenth (a twentieth on GPT-6.1 Sol
   and Terra). The summary's and My Work's instructions are shorter: their breakpoint writes nothing and
   reads nothing, at no cost.
3. **The Messages API** keeps its breakpoint; an entry's own `cache_control` still wins, and at the
   request's top level it writes the whole prompt, the record included. Summary and My Work calls
   are to keep their entry for an hour (next change: a 1-hour write costs twice the input price, which the budget must count).
   In the week to 2026-11-10, 310 of the 535 summary calls came within five minutes of the one
   before and 522 within an hour. In input prices per instruction token a miss costs 1.25 with five
   minutes' entry and 2 with an hour's, a read 0.1: the summary's instructions would cost about a
   quarter of what they do now, and My Work's about nine tenths: its reviews and placements, minutes
   to an hour apart, gain more than its rules, stories and goals lose, which come hours apart and
   pay the dearer write each time. The curator's calls come minutes apart, where five minutes' entry is the
   cheaper, and it keeps that.

A prompt's text does not change, so a held window's request (`sent`, its hash) is the same.

## How far caching goes

A window's lines are new in every call: each record is sent once. What a later call can read from
the cache is only what repeats: the instructions, and the claims carried in and the candidates,
which fit in a fifth of the window's budget and are listed newest first, so their bytes move with
each window. Their share of a prompt is the ceiling of a one-call-per-window design.

claude-mem's observer reads more than 90% of its prompt from the cache (estimated from the cost
OpenRouter reported for one call on this PC on 2026-11-10: 38,357 input and 339 output tokens of
gpt-6-luna for USD 0.000718) because each session is one conversation that grows: every call sends
the whole session so far again. Sent that way, each of
oboete's records would be written once at 1.25 times and read again at a tenth by each later call
that carries it: carried by three later calls, a record would cost 1.55 times what it costs now.
oboete keeps one call per window.

## Measuring

```sh
sqlite3 -readonly ~/.oboete/providers.db "SELECT provider, role, COUNT(*), SUM(prompt_tokens),
  SUM(cached_tokens), ROUND(100.0 * SUM(cached_tokens) / SUM(prompt_tokens), 1)
  FROM provider_calls WHERE outcome = 'ok' AND ts > <the install, unix ms> GROUP BY 1, 2;"
```

`prompt_tokens` counts every input token, those written to and read from the cache too
(`provider::usage_anthropic`; OpenAI's `prompt_tokens`).

The CLIs now read the instructions as system text: compare each entry's outcomes in the week
after the install with the week before, the answers `curate::check` refused (`unanchored`,
`shape`, `prose`) and those that did not parse (`invalid`) among them:

```sh
sqlite3 -readonly ~/.oboete/providers.db "SELECT provider, role, outcome,
  SUM(ts < <the install, unix ms>), SUM(ts >= <the install, unix ms>)
  FROM provider_calls WHERE ts >= <the install, unix ms> - 7 * 86400000
  AND ts < <the install, unix ms> + 7 * 86400000 GROUP BY 1, 2, 3;"
```
