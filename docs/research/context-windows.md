# Context windows per model id (R12 / R13, task T010)

Purpose: `src/injection/budget.ts` computes the prompt-submit character budget as
min(channel cap, `context_fraction` × documented window). The window comes from this table, keyed by the
model id the agent reports at runtime. A model missing here uses the smallest verified window of its agent
and the pack carries `Degraded: window_unknown`; an agent with no verified window at all is blocked
(R13). Every number below was collected from a primary source and survived two independent refutation
passes on 2026-09-03 (workflow `oboete-context-windows`, run `wf_5caf9ea6-ce9`, 25 claims, 19 survived;
the refuted ones were misattributed quotes, not wrong numbers, and are listed under "Not verified").

## Runtime id → catalog id

Agents report ids that differ from the catalog ids. Normalize before the lookup:

| agent | runtime id as reported | catalog id | rule |
|---|---|---|---|
| Claude Code | `claude-opus-5[1m]` (in `modelUsage` of `claude -p --output-format json`) | `claude-opus-5` | strip a trailing `[1m]`; the suffix is Claude Code's alias for the model's native 1M window, not a separate model |
| Codex CLI | `gpt-5.6-sol` (hook payload field `model`) | same | none |
| Grok Build | `grok-4.6-build` (`modelUsage` key of `grok -p --output-format json`) | `grok-4.6` | strip a trailing `-build`; the catalog entry `grok-4.6` carries `agent_type: grok-build-plan` |
| Pi | `gpt-5.6-luna` (`message.model` of `turn_end` in `--mode json`) | same | none |

## Verified windows

| model id | context window (tokens) | max output (tokens) | source | quoted statement |
|---|---|---|---|---|
| `claude-opus-5` | 1,000,000 | 128,000 | https://platform.claude.com/docs/en/build-with-claude/context-windows | "Claude Fable 5.1, Claude Mythos 5.1, Claude Fable 5, Claude Mythos 5, Claude Opus 5, Claude Opus 4.8, Claude Opus 4.7, Claude Opus 4.6, Claude Sonnet 5, Claude Sonnet 4.6, and Claude Mythos Preview have a 1M-token context window. A single request to any of them can generate up to 128k output tokens (max_tokens)." |
| `claude-sonnet-5` | 1,000,000 | 128,000 | same page; https://code.claude.com/docs/en/model-config | same sentence; "On the Anthropic API, Sonnet 5 always runs with the 1M context window. There is no 200K variant, no [1m] suffix to select" |
| `claude-opus-4-8` | 1,000,000 | 128,000 | https://platform.claude.com/docs/en/models/opus-4-8/overview | "Context window: 1M tokens · Max output: 128K tokens"; falls to 200K on Amazon Bedrock, Google Cloud Agent Platform, Microsoft Foundry, or with `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` |
| `claude-haiku-4-5` | 200,000 | 64,000 | https://platform.claude.com/docs/en/build-with-claude/context-windows ; https://platform.claude.com/docs/en/models/overview | "1M tokens for Claude Sonnet 5 and Claude Sonnet 4.6, and 200k tokens for Claude Sonnet 4.5 and Claude Haiku 4.5"; overview table: Context window 200K, Max output 64K |
| `gpt-5.6-sol` | 272,000 | 128,000 (Pi catalog) | https://github.com/openai/codex/blob/rust-v0.153.0/codex-rs/models-manager/models.json ; `~/.pi/agent/models-store.json` | `"slug": "gpt-5.6-sol", "context_window": 272000, "max_context_window": 872000`; Pi: `"contextWindow": 272000, "maxTokens": 128000` |
| `gpt-5.6-terra` | 272,000 | 128,000 (Pi catalog) | same two sources | same fields |
| `gpt-5.6-luna` | 272,000 | 128,000 (Pi catalog) | same two sources | same fields |
| `gpt-5.5` | 272,000 | 128,000 | `~/.pi/agent/models-store.json` (provider `openai-codex`) | `"contextWindow": 272000, "maxTokens": 128000` |
| `gpt-5.4` | 272,000 | 128,000 | same | same |
| `grok-4.6` | 500,000 | not documented (`max_completion_tokens: null`) | https://docs.x.ai/docs/models (redirects to https://docs.x.ai/developers/models, "Last updated: August 21, 2026") ; `~/.grok/models_cache.json` (fetched from https://cli-chat-proxy.grok.com/v1/models) | "Context 500k tokens" under the Grok 4.6 entry; catalog `"context_window": 500000` |
| `grok-4.5` | 500,000 | not documented | same two sources | same |

Codex's `models.json` also carries `max_context_window: 872000` for the gpt-5.6 family. The budget uses the
default `context_window` (272,000); the raised ceiling applies only when the CLI is configured for it, which
the hook payload does not reveal.

## Smallest verified window per agent (fallback for unknown models)

| agent | smallest verified window | from |
|---|---|---|
| Claude Code | 200,000 | `claude-haiku-4-5` |
| Codex CLI | 272,000 | `gpt-5.6-*` |
| Grok Build | 500,000 | `grok-4.6` |
| Pi | 200,000 | Pi can run any provider; use the smallest window in this table (`claude-haiku-4-5`) until the reported model is added |

## Not verified (do not use)

- `claude-opus-5[1m]` as a literal id: not in any Anthropic catalog; it is the alias form above.
- `grok-4.6-build` as a literal id: appears only in Grok Build runtime usage logs; no catalog or documentation
  entry carries a window under that name (mapping above).
- `grok-4.6-fast`: not listed on https://docs.x.ai/docs/models, in `~/.grok/models_cache.json`, or in the
  Grok Build user guide; treated as nonexistent.
- Claude Code's own default auto-compaction threshold (about 967K tokens on 1M models per
  https://code.claude.com/docs/en/model-config) is not the window and is not used.

## Maintenance

Add a row whenever a probe or dogfood run reports a model id that is not here (the probe harness records the
reported ids in `docs/research/oboete-contracts-probes.md`). A row needs the vendor documentation or the
agent's own catalog file as the source and the sentence or field that states the number.
