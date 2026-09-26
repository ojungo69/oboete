# Curator providers, second pass: why calls fail, which limits are real, subscriptions and Gemini (2026-09-27)

> Status 2026-09-27: v1 PR #96 (a short system prompt for the claude and codex curators) merged; v1 PR #97 (a 429 cools down until the provider's reset, Groq low reasoning, opencode-go 150 s) in review. The rest is input to the milestone 3 plan (§7, §8).

The owner asked on 2026-09-27: "要約プロバイダをちゃんと使えるようにしないと成立しない" (the product does not work unless the summarizing providers work). Keep a token limit only where it has a real reason. Research whether the way providers are wired in is right. Support Gemini for cost. Optimize summarizing on the subscriptions. The owner added that claude-mem and other OSS reach subscriptions through OAuth.

This note continues `curator-providers-2026-09-25.md` (the claude-mem subscription path, OpenCode Zen and Go).

How it was made:
- Two research workflows: the provider layer (code audit, provider limits, subscription CLIs, OSS designs, a completeness critic) and subscription OAuth. An independent verifier re-checked the claims against their sources. Of the provider claims, 25 held, 9 were wrong, 5 could not be verified and 4 were out of date. Only claims that held, or that I re-checked, are stated here as facts. §9 lists the corrections that matter.
- My own probes on 2026-09-27, all with synthetic text: the subscription CLIs in the `oboete-dogfood` user; Groq over HTTP with the owner's key (the key only in the request header); the OpenRouter key-info endpoint.
- The owner's store was read only for codes and provider digits (status, error code, Groq's "Limit N, Requested M" and "try again in" durations). No owner text was read.

## 1. Summary

- The chain logic (try the next provider on failure, a cooldown per provider, daily call budgets, the shape check `fits()`) is sound. The failures come from six gaps:
  1. one window size for every provider;
  2. rate-limit state that is forgotten within a run and between runs;
  3. no control of reasoning tokens;
  4. no idle gate for the subscriptions (spec 3.1);
  5. no token numbers recorded anywhere;
  6. the subscription CLIs sending their own long system prompts.
- Shipped on v1 tonight: gap 6 (#96), and parts of gaps 2 and 3 (#97).
- The token limit (`MAX_PROMPT_CHARS = 16,000`) is real for exactly one provider: Groq free's 8,000 tokens a request. For every other provider it is not a provider limit. As a window size it still has reasons: summary quality, memory, latency. Milestone 3 measures it (Window).
- Subscriptions: keep spawning the unmodified `claude` and `codex` binaries with their own login. Do not read or inject OAuth tokens, and do not use proxies (§5).
- Gemini: supported through its OpenAI-compatible endpoint, with some code (§6). Free or paid, and its place in the chain, are owner decisions (§10).

## 2. Why the owner's calls failed (store, 2026-09-22..26, codes only)

| Measured | Cause | Fix | Status |
|---|---|---|---|
| groq 429 ×25, all "tokens per day" (49 daily-token rows over groq and groq-20b in total) | Groq free gpt-oss: 30 RPM, 1K RPD, 8K TPM, 200K TPD per model (console.groq.com/docs/rate-limits, checked 2026-09-27). All 49 bodies say `try again in NmN.Ns`. The body parser read only a bare `N.Ns`, so the cooldown was the flat 45 s and Groq got the next window again long before its reset. | Parse h/m/s/ms; cool down for max(reset, 45 s) | #97 (per run); across runs in §7 |
| groq 413 ×3, groq-20b 413 ×2 | 8K TPM is also a ceiling per request. The five requests were 8,202-8,621 tokens against 8,000. With `max_completion_tokens` unset, Groq counts about the prompt only. groq-20b has the same ceiling, so the same prompt fails twice. | Pre-flight skip by estimated tokens; after a 413, skip the providers with the same ceiling | §7 |
| groq-20b 400 ×20 (at least 6 `json_validate_failed`) | Open. Strict mode is documented as constrained decoding. #91 now keeps the codes, which will show the pattern. One more `json_validate_failed` from groq 120b in tonight's replay. | Measure (§8) | open |
| nim "content is not JSON" 11 of 11 | Reasoning used up `max_tokens` 2000 and cut the JSON off | `enable_thinking: false`, `max_tokens` 4000 | #93 (merged) |
| opencode-go timeouts at 90 s; answers took 63 s on average | glm-5.3-flash reasons first | 150 s; test a thinking-off parameter | #97; §8 |
| codex 12 successes | Windows fell through the free providers; no idle gate (spec 3.1 is not in v1); each call carried codex's own instructions | #96; idle gate and persisted cooldowns | #96 merged; §7 |
| no token numbers | `provider_calls` has `ts, provider, outcome, ms, detail` only | usage columns | §7 |

`observe` runs detached from the hook, so the time lost on failed attempts delays the backlog; the owner does not wait on it. The real costs are quota, money and exposure: every failed attempt still uploads the whole window to that provider.

## 3. Which limits are real

| Limit | Real? | Keep as |
|---|---|---|
| Groq free 8K tokens per request (prompt plus any declared output) | yes, a ceiling | per-provider `max_request_tokens`, checked before the call |
| Groq 200K TPD and 1K RPD; OpenRouter free 1,000 requests a day (the owner's key is not free-tier: `is_free_tier: false` from `/api/v1/key`, 2026-09-27); OpenCode Go per-model windows; subscription 5-hour and weekly windows | yes, throughput | budgets and cooldowns, not a window size |
| `MAX_PROMPT_CHARS 16,000` as a limit for all providers | no, except Groq free, where it is too large for a Japanese-heavy window | drop as a provider limit |
| Window size for summary quality, memory and latency | yes (spec 3.1 "Window size is an evaluation variable") | one `window_tokens`, set by measurement Window at milestone 3 |
| `MAX_OBSERVATIONS`, `MAX_SUMMARY_CHARS`, `MAX_RESPONSE_BYTES` | yes: they bound output, resent context and memory | keep |

Tokens, not characters: o200k-family tokenizers spend roughly 0.7 tokens per Japanese character and 0.2-0.25 per English character (secondary source; a per-provider calibration from recorded usage absorbs the error). So the same 16,000 characters are about 3-4K tokens for an English or code-heavy window and about 11K for a Japanese-heavy one.

## 4. Measurements, 2026-09-27

### 4.1 Subscription CLIs (dogfood user, synthetic 12,000-character window)

| Variant | claude haiku, input tokens | codex gpt-6-luna (effort low), input tokens |
|---|---|---|
| v1 before #96 | 11,577 (×2) | 12,468 / 12,470 |
| CLI system prompt replaced (#96) | 5,316 (×2) | 8,990 / 8,989 |
| claude without `--json-schema`, JSON read from the text (a candidate for design B) | 4,617 / 4,617 / 4,618, 1 turn, 3 of 3 parsed | - |
| + 14 KB `~/.claude/CLAUDE.md` and an 8 KB rules file | no change (`--setting-sources ""` keeps them out) | - |
| + 7.4 KB `~/.codex/AGENTS.md` | - | +1,394. codex loads the global AGENTS.md with no setting to skip it (`codex-home/src/instructions/mod.rs`); `project_doc_max_bytes=0` covers project files only |
| + codex `include_*_instructions=false` (7 keys) | - | −461 (4%); not adopted |
| + codex tool features disabled | - | −870 (8%), each answer also had an `error` item; not adopted |

- claude with `--json-schema` sometimes takes 3 turns instead of 2 (one of the runs). Without the schema it was 1 turn every time, and 3 of 3 answers parsed. Whether design B drops the flag is still open: the milestone 1 isolation spike found that it adds `StructuredOutput` to `init.tools` (docs/spike/curator-isolation.md), which spec 6.5 would discard, and spec Appendix C item 1 leaves the decision to the milestone 3 curator spike. These numbers are input to it; three answers do not show that answers stay valid without schema enforcement.
- codex answered in 2 turns in about 3 of 10 calls, which doubles its input. The cause is not settled.
- `model_instructions_file` works with ChatGPT sign-in on gpt-6-luna (codex-cli 0.157.0). OpenAI's 2025 position was that custom instructions are for API-key auth (openai/codex#4433, closed 2025-10-01). If codex starts refusing it, the call fails and the chain moves on.
- Prompt caching gave no reads on repeated claude calls, and the fixed part of oboete's own prompt (about 250 tokens of instructions, a 413-byte schema) is below every provider's cache minimum. Chasing cache hits is not worth it; not sending what is not needed is.

### 4.2 Groq (owner's key, synthetic text)

- `reasoning_effort: "low"` is accepted on gpt-oss-120b and 20b (documented values low, medium, high). On a short window: reasoning tokens 533 -> 9 (20b) and 386 -> 75 (120b), valid JSON, about half the latency. Reasoning tokens are output tokens and count against TPD.
- Groq `qwen/qwen3.8-27b` is a third free model with its own 200K TPD (same key, same recipient). With `reasoning_effort: "none"` and strict json_schema, on the 12,000-character window asked for Japanese: 0.9-1.2 s, 3,585 prompt tokens, valid JSON in Japanese, 3-4 observations (2 of 2; 1 of 1 at 6,000 characters).
- Replay of a two-session synthetic fixture through the built binary with groq, groq-20b and mistral: both sessions summarized; one groq `json_validate_failed`, taken over by groq-20b.
- Quality at low reasoning versus the default was not compared: the night baseline had used up the TPD. The milestone 3 quality lines decide.

## 5. Subscriptions and OAuth

What claude-mem does (its bundled `worker-service.cjs`, read 2026-09-27):
- It runs the user's own installed `claude` binary through the Agent SDK (`pathToClaudeCodeExecutable`: `CLAUDE_CODE_PATH`, or the newest installed claude). Node or Bun is used only when that path is a `.js` file.
- It sends an empty `systemPrompt`. Anthropic's SDK docs say an unset system prompt still gets "a minimal prompt that covers tool calling", unlike `claude -p`, which uses the Claude Code system prompt by default. #96 gets the same saving with `--system-prompt`.
- It resumes one conversation per session (`resume: memorySessionId`). For a summarizer that sends a fresh transcript chunk each time, resuming adds the earlier turns as extra (cache-read) input instead of removing any. Not adopted. How cache reads count against Pro/Max usage limits is not published.

Terms, three shapes, for one owner using their own subscription (checked 2026-09-27):
1. **Spawn the unmodified CLI with its own login** (oboete's `claude -p` and `codex exec`; claude-mem too). Lowest risk. Today's `code.claude.com/docs/en/legal-and-compliance.md` says it does not prevent "an end user from signing in to the unmodified Claude Code binary with their own Claude subscription". It also says usage limits "assume ordinary, individual usage", and that developers building products should use API keys. The February 2026 sentence that banned OAuth use "including the Agent SDK" was on the page (Wayback 2026-02-23) and is gone now. No prohibition was found for codex with ChatGPT sign-in; OpenAI recommends API keys for automation.
2. **Read the OAuth token and pass it on** (claude-mem injects `CLAUDE_CODE_OAUTH_TOKEN` into the child). Higher risk: the same page says developers "may not collect, store, or intermediate" Claude.ai credentials. It also puts a long-lived secret into another process's environment, which spec 6.4 forbids. Not adopted.
3. **A proxy that calls the vendor backend with the token** (CLIProxyAPI style). Highest risk. It is the shape that was enforced against (OpenCode issue #6930; OpenCode PR #18186 removed its built-in Anthropic OAuth, merged 2026-03-19). Gemini CLI's own terms forbid "using third-party software" with Gemini CLI OAuth and name suspension. Not adopted.

So "the OSS use OAuth" is true, but the safe form is the one oboete already has: the vendor's binary holds the login, oboete never touches it.

## 6. Gemini

- Endpoint: `https://generativelanguage.googleapis.com/v1beta/openai` (OpenAI-compatible; beta), key file `GEMINI_API_KEY.md` (token on line 2). The owner has no such file yet.
- Terms (ai.google.dev/gemini-api/terms, effective 2026-03-23, checked 2026-09-27): unpaid use may be used "to provide, improve, and develop Google products", human reviewers may read it, and "Do not submit sensitive, confidential, or personal information to the Unpaid Services." Paid terms apply only to a project with active billing, which is billed at paid prices.
- Price (ai.google.dev/gemini-api/docs/pricing, checked 2026-09-27): `gemini-3.1-flash-lite` is free on the free tier, and USD 0.25 in / 1.50 out per million tokens paid. At about 15 windows a day (72 successes in 5 days in the owner's store) and roughly 8-11K tokens in, 1.5K out per window, that is about USD 2-2.5 a month.
- Free per-model limits are not published; AI Studio shows them for the owner's project.
- Code needed (not config only):
  1. `error_code()` must also read an error body whose root is an array (`[{"error": {...}}]`).
  2. A 429 carries its delay in `error.details[RetryInfo].retryDelay`, not in `Retry-After`. A daily quota (`quotaId` containing `PerDay`) should cool down to the next reset, not for the short `retryDelay`.
  3. Classify on the 429 status first. claude-mem checks body markers before the status and so drops the retry delay (`GeminiProvider.ts`); do not copy that.
- Chain position is a privacy decision: a provider sees every window that the providers before it failed on, not only the windows it answers.

## 7. Next, in order

On v1 (each small; the owner's request widens "v1 only for loss or safety" to provider reliability, reported to the owner):
1. **Groq qwen3.8-27b** as a third free entry (`reasoning_effort: "none"`), after groq-20b. Config only.
2. **Persisted provider state**: a `provider_state` table (`CREATE TABLE IF NOT EXISTS`), loaded by `Chain::new`. A provider whose cooldown has not passed is skipped without a call. Also: a circuit breaker after 3 consecutive invalid answers or timeouts (30 min), and 429 backoff that doubles up to 1 h when no reset is given. This is spec 3.1 (C1) for every provider, not only the subscriptions.
3. **Usage columns** on `provider_calls` (`prompt_tokens`, `completion_tokens`, `cached_tokens`, `reasoning_tokens`; `ALTER TABLE ... ADD COLUMN`, nullable): from `usage` in OpenAI-compatible answers, claude's JSON result, and codex `--json` `turn.completed.usage`. Also the digits of a Groq 413.
4. **Pre-flight `too_big`** for entries with `max_request_tokens` (Groq 8000): an estimate by script (CJK and other characters), calibrated per provider from item 3. No call, no upload, no budget spent. After a real 413, skip the providers with the same ceiling for that window.
5. **Idle gate for the subscriptions** (codex, claude, opencode-go): only when no hook has fired for 10 minutes (spec 3.1). The check reads the newest hook time right before each subscription call. On v1 every hook writes `oboete.db`, which `observe` reads. In Design B the claude and codex hooks write only `raw.db` (`capture::PORTED`, `hook::run`), so the gate there needs a heartbeat that every hook writes, or must read both stores (Codex review on #98); otherwise the window stays pending ("waiting for the owner"). A later run retries it. `observe --wait-ms 600000` alone is not a gate: it sleeps once from the start, a hook during the sleep does not extend it, and after it `observe` checks only its 60-second settle (Codex review on #98).

Milestone 3 (Design B): the window cut in estimated tokens together with measurement Window; token budgets next to call budgets, and `monthly_usd` for paid entries; proactive skips from Groq's `x-ratelimit-remaining-*` headers (they count RPD and TPM; there is no TPD header); an egress ledger of every attempt, failed ones included; Gemini error handling; claude `stream-json` with `rate_limit_event` (C1); failure kinds for invalid answers (empty, prose, wrong shape).

## 8. Measurements still to run

- Size sweep: {4K, 8K, 12K, 16K, 24K} characters × {Japanese-heavy, English/code-heavy} × each provider, at least 4 samples per cell. Groq runs serially and spread over days (TPD).
- groq and groq-20b: `reasoning_effort` low versus default, temperature 0.2 versus 1.0 (the `json_validate_failed` question).
- opencode-go with and without a thinking-off parameter.
- Pass lines: at least 95% valid answers at a size; p95 latency at most half the timeout; in replay, at most 0.5 failed attempts per success.

## 9. Corrections from verification that matter here

- The mem0-style "silent ok" (valid JSON of the wrong shape counted as success) is already fixed by `fits()`.
- OpenRouter's `models` fallback and `require_parameters` are already in the default entry.
- NIM does not need `guided_json`: its failures were truncation by reasoning, fixed in #93.
- The Mistral free-tier token numbers (e.g. "500K TPM") have no primary source now; the help article is gone. Mistral free may train on input unless the owner opts out in its Admin Console.
- OpenCode Go limits are per model (5-hour 20%, weekly 50%, monthly 100% of the model's monthly limit), not one dollar pool.
- GitHub Models was retired on 2026-07-30. Not a candidate.
- The Gemini free tier now includes 3.x Flash and Flash-Lite models; 2.5 is no longer the current free line.

## 10. Owner decisions (cost, privacy, direction)

1. **Gemini**: free (training and human review, and the terms ask not to send confidential or personal information, while transcripts can hold client code), paid (about USD 2-2.5 a month), or not at all. If used: before the subscriptions (spares their quota, but it sees more windows) or after them.
2. **Groq Developer tier** (billed per token; 250K TPM removes the 413s and the daily-token stops): about USD 1 a month at the owner's volume (the research estimate; UNVERIFIED until usage is recorded). Or stay free, with Groq taking only windows that fit.
3. **Providers that may train** (OpenRouter free, Mistral free before the opt-out, Gemini free): may windows fall through to them at all.
