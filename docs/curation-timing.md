# When a window goes to the curator (2026-10-17; decision 56, 2026-10-25)

Decision 50 asks oboete to stand in for claude-mem almost completely. One difference the owner
notices is how soon work becomes a card: claude-mem's observer makes an observation per tool call,
oboete makes cards per curation window. This note says why claude-mem works that way, what that
would cost oboete, and what oboete changes. The data behind it (claude-mem 13.40.0's source and
history, oboete's code, and counts from this PC's stores and logs) is kept outside the repository,
in `../oboete-work/cadence-2026-10-17.json`.

## Before

A window closes when it is full (`[summary] window_tokens`, 5,000 estimated tokens), or, when it
reaches the device's newest record, after `[summary] idle_minutes` (default 10) pass with no hook
record. The reply is usually the last record before a pause, so the median from a turn's end to
its card was the idle wait itself: 614 s (p95 3,217 s) on this PC.

## Why claude-mem observes each tool call

- Its hook never waits for a model: each tool call is queued at once, so the unit that reaches the
  observer is one tool call (stated in its source).
- Its Claude observer is one long model conversation per session, each queued call one more user
  message, kept byte-identical so the prompt cache is reused: an incremental prompt is cheap on
  that path (stated). Its providers without that cache replay the whole history per call, and its
  Codex observer batches for that reason (stated).
- The feed is paced to one unanswered prompt after an unpaced feed livelocked, so a session's
  observer is serial (stated). Batching for the Claude observer arrived in 13.40.0, opt-in.
- Nothing in its documents, changelog or commits gives a quality reason, such as better cards,
  for observing per call; it treats the volume as a cost and backlog problem (subagent and Bash
  filters, the bare-prompt call dropped, batching).

## What a per-call cadence would cost oboete

oboete's curator calls are stateless and go down a chain of providers with fallback, one call in
flight per device. One call per tool call would be 4.4 times today's calls (4,626 against 1,041 a
day over the last 7 days) and about 2.4 times the input tokens, three quarters of it the fixed
prompt sent again each time; the serial time it needs (9.7 to 38.5 hours a day) exceeds the day,
so cards would come later, not sooner. Copying claude-mem's cached long conversation would give up
the provider chain.

## What changed on 2026-10-17

A window in which every session's last hook record ends a turn, a reply (the agent's Stop) or a
session's end, goes at once instead of after the idle wait: nothing more of those turns is to come.
The record must be in the window as it was read: a reply appended after the window was read, or
past where the window stops, is the next window's, so a turn is not sent in two pieces. A window
in which a session's last hook record is anything else (a prompt, a tool call) waits as before,
as when one session replies while another one, its records in the same window, is still at work;
the idle wait stays the fallback for a turn that ends without a reply. A replay of this PC's last
22 days under the first form of this rule (the device's newest hook record) gave about 20 more
calls a day (1-2%), the extra windows being short; the expected lag from a reply to its card is
one curator call (median 6.8 s) plus any queue.

## Not changed, and next

- Per-call observation, for the cost above (decision 56 decided otherwise: below).
- The queue of windows closed by their size (median 1,051 s, p95 3,776 s behind earlier windows
  and provider holds) is measured before anything is changed for it; any change that spends money
  goes to the owner with its numbers first.
- After release, the lag from a reply to its window op is measured again on this PC.

## Decision 56: a window per step (2026-10-25)

Decision 56 asks for cards as claude-mem makes its observations, one per tool call, before the
switch. What is built:

- While the owner is at work (a hook record within `[summary] idle_minutes`), a window that is not
  full goes as soon as its work is done so far: it holds a done step (a tool call, a reply, a
  session's end), and no session's prompt or harness envelope in it still waits for that session's
  work. Such a start, and every record after it, is left to the next window, which waits for that
  work, so a prompt is curated with what it asked for. A memory tool's call is no step. Once the
  owner stops, a window goes whole, as before. A span sent again by a recuration is cut as before.
- Still one call at a time for each device: the records that arrive while a call is out make the
  next window, so under a backlog a window holds several steps, as claude-mem's Codex observer
  batches what is already queued. Each window is still one stateless call down the provider chain;
  claude-mem's Claude observer keeps one model conversation per session instead.
- A window that every provider failed keeps its hold while it only grows (the same chain, rules,
  language and records up to where it was held): each step would otherwise send it again before
  the providers that failed it are due. Another chain is asked at once, as before.
- A run asks for the oldest turn summary due first, and for a window only when it sent none
  ([summaries.md](summaries.md) T1): windows going a step at a time would otherwise hold summaries
  back for as long as the work goes on.
- The prompt says that a step or two usually make one card or none, after claude-mem's per-step
  rule of what to skip.

A replay of this PC's hook records from 2026-10-15 to 2026-10-25 under both rules, with one call
in flight and call times drawn from the Anthropic entry's (the script and its output are kept
outside the repository, in `../oboete-work/cadence54/`):

| | 2026-10-17 rule | decision 56 |
|---|---|---|
| Curator calls a day (mean) | 1,416 | 2,549 |
| Tool call to its window's answer, median | 79 s | 34 s |
| The same, p90 | 793 s | 826 s |
| Windows holding several tool calls | 59% | 28% |
| Windows within an 8,000-token request (Groq's limit) | 5% | 47% |
| Call time a day, one call at a time | 2.1 h | 3.9 h |

The p90 is the queue behind earlier windows on the busiest days (2026-10-21: 7,397 calls). On
2026-10-24 the curator answered 530 windows, and the replay gives that day 1,787, about 3.4 times.

Next: the Anthropic entry's prompt caching, as the instructions are the same bytes for every window
(G09); after release, cards a day against claude-mem's observations (about 1,300 a day on
2026-10-23 and 2026-10-24) and the lag from a tool call to its window op, measured again on this PC.
