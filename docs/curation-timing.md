# When a window goes to the curator (2026-10-17)

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

## What changes

A window whose newest hook record ends a turn, a reply (the agent's Stop) or a session's end, goes
at once instead of after the idle wait: nothing more of that turn is to come. A window whose newest
hook record is anything else (a prompt, a tool call) waits as before, and the idle wait stays the
fallback for a turn that ends without a reply. A replay of this PC's last 22 days gives about 20
more calls a day (1-2%), the extra windows being short; the expected lag from a reply to its card
is one curator call (median 6.8 s) plus any queue.

## Not changed, and next

- Per-call observation, for the cost above.
- The queue of windows closed by their size (median 1,051 s, p95 3,776 s behind earlier windows
  and provider holds) is measured before anything is changed for it; any change that spends money
  goes to the owner with its numbers first.
- After release, the lag from a reply to its window op is measured again on this PC.
