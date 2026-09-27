# Spike: curator window sizes per provider (2026-09-27)

The size sweep of docs/research/curator-providers-2026-09-27.md section 8, first run. It feeds docs/milestone-3-plan.md D7 (the token estimate) and D8 (the interim window size). It does not decide Window: that is measurement Window at the end of milestone 3, on dev transcripts with M2, M3 and M6 (spec 8.2).

## How

- Synthetic windows only, no owner data: a coding session about a date parser, repeated with varied file names and numbers, cut at {4,000, 8,000, 12,000, 16,000, 24,000} characters, Japanese-heavy (Japanese dialogue, English paths and tool lines) or English and code. Seeds vary per sample.
- The prompt, schema, request body (strict `json_schema`, temperature 0.2) and each entry's model and extra fields are v1's at 4bfe458 (`observe::build_prompt`, `provider::openai_compat`, `config::default_providers`), asked to answer in Japanese as the owner's config does.
- HTTP entries with the owner's keys (read in-process, sent only as the header); claude and codex in the dogfood user with v1's flags. Up to 4 samples a cell; codex 2 samples at 3 sizes, to spare the ChatGPT window the Codex review lane shares. Timeouts were 300-400 s so that slow answers were measured, not cut.
- An answer is valid when its text (a markdown fence removed) is JSON that fits the schema, as the chain's `fits()` checks.

## Results

| Provider | Answers | Valid | Median s | p95 s | Max s | Chain timeout | Failures |
|---|---|---|---|---|---|---|---|
| nim (nemotron-3-super, thinking off) | 40 | 40 | 4.8 | 11.3 | 15.6 | 90 | - |
| openrouter (nemotron-3-super :free) | 40 | 39 | 17.3 | 68.7 | 314.6 | 90 | 1 empty answer after 315 s |
| opencode-go (glm-5.3-flash) | 40 | 34 | 50.1 | 71.8 | 235.6 | 150 | 6 prose |
| codex (gpt-6-luna, effort low) | 12 | 12 | 9.4 | 12.8 | 24.5 | 180 | - |
| claude (haiku) | 40 | 40 | 17.4 | 36.7 | 41.6 | 180 | - |
| groq (gpt-oss-120b) | 3 | 2 | 1.7 | - | 1.8 | 90 | daily tokens spent |
| groq-qwen (qwen3.8-27b) | 4 | 2 | 1.2 | - | 1.3 | 90 | 1 tokens-a-minute 429; 1 413 at 16,000 Japanese characters |
| groq-20b (gpt-oss-20b) | 2 | 0 | - | - | 0.8 | 90 | 1 `json_validate_failed`, then daily tokens spent |
| mistral (mistral-small) | 40 | 0 | - | - | 1.4 | 90 | 40 × 429: the key allows 0 requests a minute |

- **Size does not decide validity or latency** for any entry but Groq: every other entry answered 24,000 characters (about 12,800 tokens Japanese-heavy) as validly and about as fast as 4,000. Latency depends on the model (thinking), not on the window.
- **Groq**: the owner's own curation had spent the daily tokens of gpt-oss-120b and 20b before the sweep, so only a few cells ran; the rest waits for their daily reset (resumable, `sweep.py`). Groq free's 8,000-token ceiling refused 16,000 Japanese-heavy characters with 413, as expected.
- **Mistral**: every request is refused with 429 before any processing. The key's headers say `x-ratelimit-limit-req-minute: 0`: the key itself is shut (the free plan not enabled on it), not a burst. The body's fields are at its root (`type: rate_limited`, `code: "1300"`) with no reset.
- **OpenCode Go** answered prose instead of JSON 6 times in 40 (85%), under the 95% pass line, although it accepts strict `json_schema`. A follow-up of 4 calls was 4 of 4 valid, each a fenced JSON. It reasons first (5,000-9,000 characters of `reasoning_content` a call), which is why it takes about 50 s. Of the thinking-off parameters, `thinking: {type: disabled}` and `reasoning_effort: none` are refused (400), and `chat_template_kwargs: {enable_thinking: false}` is accepted but changes nothing (still 6,800-9,200 characters of reasoning, 47-59 s). No way to turn it off was found.
- **OpenRouter** answered 39 of 40, but one call hung 315 s and returned nothing, and its p95 of 69 s is over half its 90 s timeout.
- **nim** answered all 40, at a median of 5 s: since #93 (thinking off, 4,000 max tokens) it is the fastest valid free entry. The owner's store had 0 of 26 nim answers valid before that fix.

Pass lines (research note section 8: ≥ 95% valid, p95 ≤ half the timeout): nim, claude and codex pass; OpenRouter fails on latency; OpenCode Go fails on validity; Groq is not yet measured; Mistral cannot be measured.

## Tokens per character (D7)

Least squares of each answer's reported `prompt_tokens` on the prompt's CJK and other characters:

| Provider | Per CJK character | Per other character | Fixed |
|---|---|---|---|
| nim | 0.81 | 0.28 | -43 |
| openrouter | 0.81 | 0.28 | -1 |
| opencode-go | 0.79 | 0.26 | -38 |
| claude (v1 flags) | 0.88 | 0.32 | 1,672 |
| codex | 0.80 | 0.26 | 5,423 |

- The API entries agree: about 0.8 per CJK character and 0.28 per other character, with no fixed part. `budget::estimate` uses these two coefficients. The earlier guess (0.7 and 0.25, research note section 3) counted about 12% low.
- The subscription CLIs add their own fixed part: codex its instructions and tool definitions (about 5,400 tokens), claude its system prompt under v1's flags (about 1,700; with Task 2's flags a whole call read 512). The per-provider factor absorbs the rest (`budget::factor`); only Groq has a ceiling, and Groq is an API entry.

## For D8 (the interim window)

Only Groq limits the window: every other entry took 24,000 characters. So the interim `window_tokens` is what fits Groq with the prompt's fixed part: 6,000 estimated tokens (about 7,500 characters of pure Japanese, 10,000 of the Japanese-heavy mix above, or 21,000 of English), under 95% of 8,000 with room for the instructions and an earlier summary. Measurement Window replaces it.

## Next

- Groq's cells, after its daily reset, 4 samples each, serially (also `reasoning_effort` low against the default and temperature 0.2 against 1.0 for the `json_validate_failed` question, research note section 8).
- The chain order is the owner's (2026-09-27: groq, groq-20b, groq-qwen, openrouter, mistral, nim, opencode-go, codex, claude). These numbers suggest nim before OpenRouter and Mistral out until its key is enabled; the owner decides.
