# Curator providers: the Claude subscription the claude-mem way, OpenCode Zen and Go (2026-09-25)

> Status 2026-09-26: adopted in part. PR #56 took grok out of the default chain, PR #58 moved the subscription curators to cheap models (claude `haiku`, codex `gpt-6-luna`) and added the shape check in the chain (§3.4 item 1). The owner already subscribes to OpenCode Go (owner decision 25 in `docs/spec.md`); the rest of §2.3 and §3.4 is specified in `docs/spec.md` for the redesign.

This note answers owner decision 23 (RD/owner-decisions.md:32), items 1 and 3: "To research and adopt if possible: how claude-mem uses the Claude subscription, and OpenCode's free models and OpenCode Go models as providers." Item 2 (the grok subscription is no longer used for curation, judge or digest) needs no research. Its edits to the spec are listed in §4.

How it was made. Two researchers wrote notes. An independent verifier re-checked every claim against its source. Only verified claims are stated as facts here. Claims that failed verification are in §5 with the reason. Where I checked something myself while writing, it says "checked 2026-09-26 while writing".

Rules kept: no agent CLI was run with a model (only `claude --version` and `claude --help`). No key or token file was read; only file names and modes were listed. The OpenCode probes were plain `curl` calls with synthetic text.

Short names:
- `RD/` = docs/research/redesign-2026-09-24/ in this repo. oboete code is at main 96cc106.
- `CM/` = the local claude-mem checkout `~/.claude/plugins/marketplaces/thedotmack/`. It is shallow at `02cd0c9c47e38a849e764477290c571e84dfa043` (`git log -1 --format=%H`), which equals upstream main. `hardened-options.ts` has not changed since c4bfa45, the commit section 6 cites: its newest commit is "ed2b39b4 2026-09-12T17:16:34Z harden(observer): deny SendMessage and ListAgents to Observer sessions (#3709)" (`gh api repos/thedotmack/claude-mem/commits?path=src/sdk/hardened-options.ts`, checked 2026-09-26).
- `CMB` = the Agent SDK bundle that claude-mem actually runs: `~/.claude/plugins/cache/thedotmack/claude-mem/13.25.3/scripts/worker-service.cjs`.
- `CC/<page>` = `https://code.claude.com/docs/en/<page>.md`. Line numbers refer to that markdown file. Checked 2026-09-26.
- `PC` = `https://platform.claude.com/docs/en/build-with-claude/prompt-caching.md`. Checked 2026-09-26.
- `OZ` = https://opencode.ai/docs/zen, `OG` = https://opencode.ai/docs/go, `TOS` = https://opencode.ai/legal/terms-of-service, `PP` = https://opencode.ai/legal/privacy-policy. Checked 2026-09-25; the verifier re-read them on 2026-09-26.
- `OC/` = https://github.com/anomalyco/opencode/blob/34aa427434b054afcce7184764aa681159b5d769/ (the OpenCode repo, pinned).

## 1. Summary and recommendation

**Claude subscription.**
- claude-mem has no special subscription channel. Its Agent SDK starts the user's own `claude` binary. That binary logs in with the user's `/login` credential. oboete's `claude -p` already does exactly this.
- Do not adopt the Agent SDK, claude-mem's token injection, or its long-lived streaming session.
- Adopt four groups of flags in `headless_command("claude")` (src/provider.rs:366-390): a short fixed system prompt; `stream-json` output with a check on every call and a persisted cooldown; the permission lockdown; and the environment names that S8's allow-list must never pass.
- Keep the subscription as an explicit opt-in in public setup (decision 21). The policy page read on 2026-09-26 also says an end user may sign in to the unmodified binary with their own subscription. Section 7 had withdrawn that sentence (§4).
- Before shipping, one dogfood call must settle three points (§5). Two points are owner decisions: how much of the allowance background work may use, and the model.

**OpenCode.**
- Zen free models: do not add. Two of three refused a plain API call with HTTP 403 ("OpenCode's free tier can only be used from within OpenCode"). The third ignored the JSON schema.
- OpenCode Go: can be added, but only as an explicit opt-in subscription, never in `default_providers()`. Model `glm-5.3-flash`.
- Before Go goes in, the owner answers three questions (usage policy and terms, the $10 a month, whether decision 21 covers an API-key subscription). oboete also needs three code changes (§3.4).
- The `opencode` CLI as a curator: no. The API path makes it unnecessary.

## 2. Claude subscription: claude-mem vs oboete

### 2.1 How claude-mem reaches the subscription

- The SDK runs the CLI: "A library that runs the Claude Code binary, with Claude Code's" (CC/agent-sdk/overview line 17).
- claude-mem feeds it a streaming generator: `prompt: messageGenerator,` (CM/src/services/worker/ClaudeProvider.ts:308).
- When the OS credential store returns a token, claude-mem puts it into the child's environment: `isolatedEnv.CLAUDE_CODE_OAUTH_TOKEN = result.token;` (CM/src/shared/EnvManager.ts:288).
- On Linux it asks libsecret: "Linux libsecret lookup failed (is secret-tool installed?)" (CM/src/shared/oauth-token.ts:353). On this machine `command -v secret-tool || echo "secret-tool: not found"` printed "secret-tool: not found" (2026-09-26). So here nothing is injected.
- Then the CLI uses its own login: "7. Subscription OAuth credentials from `/login`. This is the default for Claude Pro, Max, Team, and Enterprise users." (CC/authentication line 209). It is last in the list, so any other credential left in the child's environment wins over it.
- The CLI's own login file: "On Linux, credentials are stored in `~/.claude/.credentials.json` with file mode `0600`." (CC/authentication line 173). It exists here with mode 600; the contents were not read.
- Neither tool has used the Claude subscription on this machine. claude-mem is set to `"CLAUDE_MEM_PROVIDER": "openrouter"` (~/.claude-mem/settings.json). oboete has never called claude, because groq answers first (`provider_calls`: agy 3, `groq|19`, groq-20b 4, workers-ai-embed 13; no claude row).

### 2.2 The five areas

| Area | claude-mem (Agent SDK) | oboete (`claude -p`) | Take |
|---|---|---|---|
| 1. Auth path | SDK runs the installed `claude`. Injects the OAuth token from the OS store when found (not on WSL). Blocks the parent's token, API key and base URL, and all `CLAUDE_CODE_*`. | Runs `claude` directly. Removes names containing TOKEN, KEY, SECRET or PASSWORD. `ANTHROPIC_BASE_URL` and `CLAUDE_CODE_EFFORT_LEVEL` pass through. | The env names only (via S8). Not the injection. |
| 2. Session reuse and caching | One long-lived conversation per recorded session, one turn per tool call. Retired at 400,000 chars or 3 minutes idle. Never resumed. | One fresh call per window, in a new random temp directory. The default prompt contains that directory, so no call can reuse another's cache. | No reuse. A short fixed system prompt instead. |
| 3. Token cost per call | Empty system prompt. Haiku 4.5. Thinking off for the Observer. | Full default system prompt (illustrative 4,200 tokens). Sonnet. The usage figures are thrown away. | `--system-prompt-file`. Record usage. Model is the owner's call. |
| 4. Quota and cooldown | Reads `rate_limit_event`. Aborts at utilization thresholds. A persisted 30-minute breaker, then one probe. | An in-memory cooldown (600 s for outages) that lasts one `observe` run. Never reads rate-limit data. | Persist a cooldown until `resetsAt` on `rejected`. Thresholds are the owner's call. |
| 5. Lockdown layers | `tools: []`, a deny list, `dontAsk`, `canUseTool`, no settings, strict MCP. Holes: `--tools ""` is dropped on the streaming spawn; `canUseTool` never runs; `Agent` and `Monitor` are not denied. | `--tools ""`, `--setting-sources ""`, `--strict-mcp-config`, `--no-session-persistence`, hooks off. No permission mode, no deny list, no per-call check. | `dontAsk`, `--permission-prompts none`, a deny list plus `mcp__*`, `--disable-slash-commands`, the init check. |

**Area 1 evidence.**
- claude-mem blocks inherited credentials: `'CLAUDE_CODE_OAUTH_TOKEN', // Issue #2215: prevent stale parent-process token from leaking into` (CM/src/shared/EnvManager.ts:42). The same list holds `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and `ANTHROPIC_BASE_URL`.
- Why the base URL: "Issue #2375: same leak class as AUTH_TOKEN." (CM/src/shared/EnvManager.ts:37).
- A second filter: `export const ENV_PREFIXES = ['CLAUDECODE_', 'CLAUDE_CODE_'];` (CM/src/supervisor/env-sanitizer.ts:6). It drops every such name except a preserve list.
- oboete's filter: `if k.contains("TOKEN")` (src/provider.rs:451; the whole filter is :450-458, and also removes `CLAUDECODE`). `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_*` and `CLAUDE_CODE_EFFORT_LEVEL` contain none of the four substrings, so they pass.
- Why the effort variable matters: `CLAUDE_CODE_EFFORT_LEVEL` "Takes precedence over `--effort`, `/effort`, and the `modelSettings` and `effortLevel` settings." (CC/env-vars line 270).

**Area 2 evidence.**
- "The observer keeps one long-lived conversation per session and appends every" (CM/src/shared/observer-recycle.ts:4).
- `export const OBSERVER_CONVERSATION_MAX_CHARS = 400_000;` (CM/src/shared/observer-recycle.ts:30), about 100k tokens by its own comment.
- `export const IDLE_TIMEOUT_MS = 3 * 60 * 1000;` (CM/src/services/worker/SessionMessageBuffer.ts:5).
- `const shouldResume = false;` (CM/src/services/worker/ClaudeProvider.ts:255) and `const observerExtraArgs = ['--no-session-persistence'];` (:222).
- The SDK caches "to reduce costs on repeated content. You do not need to configure caching yourself." (CC/agent-sdk/cost-tracking line 337).
- "Unless you choose a TTL yourself, Claude Code requests the one-hour TTL only on a Claude subscription within your plan's included usage." (CC/prompt-caching line 262).
- "By default, two sessions that use the same `claude_code` preset and `append` text still cannot share a prompt cache entry if they run from different working directories." (CC/agent-sdk/modifying-system-prompts line 208). And: "In Claude Code, the cache is effectively scoped to one machine and directory." (CC/prompt-caching line 297).
- oboete's directory is new and random each call: `let dir = std::env::temp_dir().join(format!("oboete-cli-{name}"));` (src/provider.rs:316; random name from `getrandom` at :314-315; used as cwd at :440).
- Why not a long session: "so a one-line question in a session that has been open all day still draws usage for the whole conversation" (CC/costs line 354).
- Issue #54 will split long sessions into chunks, so there will be more calls per session and fixed per-call overhead matters more. Its body: "`render()` は文章を組み立てた後、16,000文字を超えると先頭8,000文字と末尾8,000文字だけを残し、中央を省略する。" (https://github.com/ojungo69/oboete/issues/54).

**Area 3 evidence.**
- "This differs from `claude -p`, which uses the Claude Code system prompt by default." (CC/agent-sdk/modifying-system-prompts line 15). The SDK itself uses a minimal prompt when none is set.
- claude-mem sets none, and the bundle sends an empty one: `i===void 0?d=""` (CMB line 2613).
- The docs show `tokens: 4200` for "System prompt" (CC/context-window lines 16-17) and warn: "Token counts are illustrative. Actual values vary with your CLAUDE.md size, MCP servers, and file lengths." (line 750).
- On the API: "1-hour cache write tokens are 2 times the base input tokens price" (PC line 265).
- Minimum cacheable size: "1,024 tokens for Claude Opus 4.8, Claude Sonnet 5, Claude Sonnet 4.6, Claude Sonnet 4.5" (PC line 608).
- Models: claude-mem `CLAUDE_MEM_MODEL: 'claude-haiku-4-5-20251001',` (CM/src/shared/SettingsDefaultsManager.ts:173); oboete `cli("claude", Some("sonnet"), 200),` (src/config.rs:211).
- claude-mem turns thinking off: `...(input.source === 'Observer' ? { thinkingConfig: { type: 'disabled' as const } } : {}),` (CM/src/sdk/hardened-options.ts:160).
- "The response includes metadata about the request (session ID, usage, etc.) with the structured output in the `structured_output` field." (CC/headless line 138). oboete keeps only `structured_output` (src/provider.rs:577-578).
- **Estimate, not measured:** each oboete call writes about 4,200 prompt tokens to the cache and never reads them back. At API weighting that is about 8,400 base-input-equivalent tokens of overhead per call. How subscription usage weights cache writes is not documented (§5).

**Area 4 evidence.**
- claude-mem reads the events: `m.type === 'rate_limit_event' || (m.type === 'system' && m.subtype === 'rate_limit');` (CM/src/services/worker/RateLimitStore.ts:118).
- Thresholds, for example `five_hour: 0.95,` (RateLimitStore.ts:180). API-key auth is exempt: `if (isApiKeyAuth(authMethod)) {` (:209).
- A weakness: `const key: RateLimitBucketKey = info.rateLimitType ?? 'default';` (:66). The abort loop checks only named windows (:213-222). The public event type has no `rateLimitType` field: its fields are status, resetsAt, utilization and error fields, for example `status: "allowed" | "allowed_warning" | "rejected";` (CC/agent-sdk/typescript line 5134).
- "When `errorCode` is `"credits_required"`, the rejection is from a claude.ai subscription whose included usage is exhausted" (CC/agent-sdk/typescript line 5146).
- The persisted breaker: `export const QUOTA_EXHAUSTED_RECHECK_COOLDOWN_MS = 30 * 60_000;` (CM/src/shared/quota-cooldown.ts:122), armed by `recordQuotaExhausted(provider, quotaMessage, reason?.split(':')[1]);` (CM/src/services/worker/http/routes/SessionRoutes.ts:499).
- "Usage counts against the session and weekly allowances at the same time." (CC/errors line 620).
- oboete: `const COOLDOWN_OUTAGE: Duration = Duration::from_secs(600);` (src/provider.rs:25). Which CLI failures get it: see §5. The chain is built per run: `let mut chain = provider::Chain::new(&cfg.providers);` (src/observe.rs:52), and its cooldowns are an in-memory map (src/provider.rs:56-59). So once claude hits its 5-hour or weekly limit, each new `observe` run tries claude again.

**Area 5 evidence** (claude-mem's holes are read from code, not run).
- The bundle turns an empty tool list into two arguments: `M.length===0?B.push("--tools","")` (CMB line 2529).
- claude-mem's spawn factory removes every flag whose value is empty: "The SDK encodes optional flag/value pairs as `--flag ''` when the" (CM/src/supervisor/process-registry.ts:715; the stripping is at :711-723).
- The streaming Observer uses that factory: `spawnClaudeCodeProcess: createSdkSpawnFactory(session.sessionDbId, slotReservation, observerExtraArgs),` (CM/src/services/worker/ClaudeProvider.ts:319).
- The SDK does not resend the tool settings another way. Its initialize request starts `subtype:"initialize",hooks:this.initHooksPayload,sdkMcpServers:e,sdkMcpServerConfigs:n,jsonSchema:this.jsonSchema,systemPrompt:` (CMB line 2531) and has no tools, allowedTools, disallowedTools or permissionMode field.
- claude-mem's docblock says the layers have no CLI form: "except `disallowedTools` is an SDK `Options` field with no command-line" (CM/src/sdk/hardened-options.ts:35). The bundle maps them anyway: `_&&B.push("--permission-mode",_)` (CMB line 2529). Its own comment agrees: "`--permission-mode dontAsk` on every Observer/KnowledgeAgent spawn (see" (CM/src/shared/find-claude-executable.ts:8).
- `canUseTool` is dead under `dontAsk`: "`canUseTool` is never called" (CC/agent-sdk/permissions line 121) and "In `dontAsk` mode, this step is skipped and the tool is denied." (line 46).
- `dontAsk` still runs some calls: "and so do calls that need no approval in `default` mode, such as file reads inside your working directories and calls to `Agent`." (CC/agent-sdk/permissions line 260). claude-mem denies `'Task',           // No spawning sub-agents` (CM/src/sdk/hardened-options.ts:70) and has no `Agent` entry.
- `Monitor` "Runs a command in the background and feeds each output line back to Claude" (CC/tools-reference line 39). It is not on claude-mem's deny list (hardened-options.ts:61-76).
- oboete today: `-p`, `--output-format json`, `--json-schema`, `--setting-sources ""`, `--tools ""`, `--strict-mcp-config`, `--no-session-persistence`, and `r#"{"disableAllHooks":true}"#,` (src/provider.rs:384; flags at :366-386).

### 2.3 What to adopt

All of it goes into `headless_command("claude")`. No new dependency, no SDK, still one call per window. Flag names were checked while writing (2026-09-26). `claude --help` (2.1.282) lists `--permission-prompts <target>`, `--disallowedTools, --disallowed-tools <tools...>` and `--disable-slash-commands`. It shows `--system-prompt-file` only inside the `--bare` text (`--system-prompt[-file]`), but the reference has its own row: `--system-prompt-file` "Load system prompt from a file, replacing the default prompt" (CC/cli-reference line 130).

1. **Replace the system prompt** with `--system-prompt-file <scratch>/system.md`. It holds a short, fixed curator identity and no recorded text. New; section 6 does not have it.
   - Basis: "Use a replacement flag when the surface, identity, or permission model differs from Claude Code's, like a non-coding agent in a pipeline that no human watches." (CC/cli-reference line 156). The same line goes on: "Replacing drops all of the default prompt, including tool guidance and safety instructions, so you take responsibility for whatever your task still needs." So the curator prompt must carry what the task needs.
   - It removes the illustrative 4,200-token default and its per-call cache write. A short prompt stays below the 1,024-token cache minimum, so it is simply not cached. It is still far smaller.
   - A file keeps prompt text off the command line (section 6 rule).
2. **Stream and check every call**: `--output-format stream-json --verbose`.
   - Check `system/init` on every call. Section 6 already requires this: "Where a CLI reports its tool list in each call's output (agy's init event; claude's with `--output-format stream-json`), the worker checks it on every call and discards the result when any tool is present." (RD/section-6-revised.md:97). What the event carries: "The `system/init` event reports session metadata including the model, tools, MCP servers, and loaded plugins." (CC/headless line 218). Check tools, MCP servers, plugins, the permission mode and `apiKeySource`.
   - `apiKeySource: "none"` proves only that no API key is used: "No API key. The session authenticates another way, such as a claude.ai login, a bearer token, or a cloud provider" (CC/agent-sdk/typescript line 4423). The environment rule (item 4) is what keeps it on the subscription.
   - New: on a `rate_limit_event` with status `rejected`, store a cooldown until `resetsAt` so it survives the run. On `errorCode` `credits_required`, stop claude until the owner acts.
   - New: record the usage and cache-token figures of each call, so area 3 is measured instead of estimated.
3. **Close the permission layers.**
   - `--permission-mode dontAsk` and an explicit `--disallowedTools` list: already in section 6 (RD/section-6-revised.md:93). The list must name both `Agent` and `Task`, and `Monitor`.
   - New: `mcp__*` in that list. "The flag doesn't affect MCP tools; to deny those too, use `--disallowedTools "mcp__*"`." (CC/cli-reference line 135).
   - New: `--permission-prompts none`. "Pass `none` when nobody can answer, and Claude Code denies them instead." (CC/cli-reference line 112). The same line says it needs v2.1.259 or later; the local CLI is 2.1.282. `oboete doctor` probes it (§4).
   - New: `--disable-slash-commands`.
4. **Environment.** Section 6 already replaces today's name filter with S8's `env_clear` plus an allow-list (RD/section-6-revised.md:76-77). That drops `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_EFFORT_LEVEL` and the other `CLAUDE_CODE_*` names by construction. This note adds one rule: no `ANTHROPIC_*` or `CLAUDE_CODE_*` name goes on the allow-list. Until S8 lands, today's filter lets those names through (area 1).
5. **Keep** the stdin prompt, the random private cwd, `--no-session-persistence`, `--setting-sources ""`, `--strict-mcp-config`, `--tools ""`, hooks off and the self-capture marker. oboete never touches the credential.

### 2.4 What not to adopt, and why

- **The Agent SDK.** It runs the same binary. Every layer except `canUseTool` maps to a CLI flag, and `canUseTool` is dead under `dontAsk` (area 5). The SDK page also says: "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK." (CC/agent-sdk/overview line 44).
- **Token extraction and injection.** It puts a credential into a child's environment, which breaks "provider API keys are read from files and never go into a subprocess environment, a command line or a log" (RD/section-6-revised.md:75). It is close to the policy's "collect, store, or intermediate" clause (quoted below). And on WSL it does nothing.
- **A long-lived session.** oboete sends one call per window, and #54's chunks are independent. A reused conversation would re-read every earlier window on every call (area 2).
- **`--bare`.** "In bare mode, Claude Code never reads OAuth credentials or the system keychain." (CC/headless line 49). It cannot use the subscription.
- **`--max-turns 1`.** Structured output retries: "the SDK validates the output against it, re-prompting on mismatch." (CC/agent-sdk/structured-outputs line 9). One turn might cut that off. Not adopted until tested.
- **claude-mem's thresholds as constants, and Haiku.** Both change how much of the owner's allowance is used or the summary quality. They are owner decisions (§5).

### 2.5 Policy lines for the opt-in (checked 2026-09-26)

Keep the Claude subscription as an explicit opt-in in public setup (decision 21). The tier line quotes:
- "Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or session tokens" (CC/legal-and-compliance line 50).
- "Nor does it prevent an end user from signing in to the unmodified Claude Code binary with their own Claude subscription" (CC/legal-and-compliance line 52). I re-fetched the page while writing (2026-09-26) and the sentence is there.
- "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of Claude Code and the Agent SDK." (CC/legal-and-compliance line 43).
- Supporting line, from the part on offering Claude Code inside a product (read while writing, 2026-09-26): "Each end user must authenticate with their own Anthropic API key, Claude subscription plan credentials, or 3P inference provider credential" (CC/legal-and-compliance line 27).

oboete's path is the second sentence: the user's own unmodified binary, the user's own login, and oboete never handles the credential. Line 43 is why the owner should set a usage threshold for background work.

## 3. OpenCode Zen and Go

### 3.1 What they are

- Zen: "OpenCode Zen is an AI gateway that gives you access to these models." (OZ). "You are charged per request and you can add credits to your account." (OZ). "If your balance goes below $5, Zen will automatically reload $20." (OZ). The same section adds: "You can also disable auto-reload entirely."
- Go: "OpenCode Go is a low cost $10/month subscription that gives you reliable access to popular open coding models." (OG). "You sign in to OpenCode Zen, subscribe to Go, and copy your API key." (OG).
- Go limits are dollars per model: "Each model has the following usage limits: 5-hour — 20% of the monthly limit; weekly — 50%; and monthly — 100%." (OG).
- Overflow: "When enabled, Go will fall back to your Zen balance after you’ve reached your usage limits instead of blocking requests." (OG). The published gateway agrees: `if (!authInfo.billing.lite.useBalance) throw e` (OC/packages/console/app/src/routes/zen/util/handler.ts#L964).
- Other agents are allowed: "While Zen works great with OpenCode, you can use Zen with any agent. Follow the setup instructions in your preferred coding agent." (OC/packages/console/app/src/i18n/en.ts#L229). "Yes, you can use Go with any agent. Follow the setup instructions in your preferred coding agent." (en.ts#L378).

### 3.2 Zen free models: do not add

- The free tier refuses plain API callers. Calls to `big-pickle` and `nemotron-3.5-lightning-free` at https://opencode.ai/zen/v1/chat/completions got HTTP 403 with body `{"type":"error","error":{"type":"FreeTierError","message":"Error from provider (Console): OpenCode's free tier can only be used from within OpenCode"}}` (probes of 2026-09-25 around 15:05 UTC; the raw responses were saved in the research session's scratch directory and are not in this repo). The message speaks of the free tier as a whole.
- The one that answered ignored the schema. `space-bunny-free` returned HTTP 200 with the keys issue, resolution and decision instead of the required title and facts, for example `\"resolution\": \"Replacing tabs with spaces fixed the build.\"`. Its own reasoning shows it never saw the schema.
- The OpenCode client uses a fixed placeholder key for free models: `options: ok ? {} : { apiKey: "public" },` (OC/packages/opencode/src/provider/provider.ts#L205). The gateway treats it as no key: `const zenApiKey = rawZenApiKey === "public" ? undefined : rawZenApiKey` (handler.ts#L104). Passing the gate would mean pretending to be the OpenCode client. We should not.
- Data use. "Big Pickle: During its free period, collected data may be used to improve the model." (OZ); MiMo-V2.6-Flash Free, MiMo-V2.5 Free and Ling 3.0 Flash Fin Free carry the same sentence. "Nemotron 3.5 Lightning Free (NVIDIA free endpoints): Trial use only — do not submit personal or confidential data. Your use is logged for security purposes and to improve NVIDIA products and services." (OZ); Nemotron 3 Ultra Free has the same text. "Space Bunny Free is a stealth model that’s free on OpenCode for a limited time. Its provider follows a zero-retention policy and does not use your data for model training." (OZ). A curator sends redacted transcripts, so most free models lower the privacy level.
- Quota: "Free models include Big Pickle plus promotional models available at the time, with a quota of 200 requests/day." (en.ts#L382; FAQ copy).
- Two free models are on endpoints oboete cannot call: "Jev 1.13 Free | jev-1.13-free | https://opencode.ai/zen/v1/systemone" (OZ), and Muse Spark 1.3 Contributor Free on `/responses`. oboete only calls `let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));` (src/provider.rs:218).
- The gate may tighten: `// temporarily disable check headers` (OC/packages/console/app/src/routes/zen/util/ipRateLimiter.ts#L12) guards a lower limit for clients without the expected headers.

### 3.3 OpenCode Go: add only as an explicit opt-in

**Findings.**
- Recommended model: `glm-5.3-flash`.
  - Price and allowance: "GLM-5.3-Flash | $0.15 | $0.50 | $0.03 | - | $60" (OG; input, output, cached read, cached write per 1M tokens, then monthly limit).
  - Endpoint: "GLM-5.3-Flash | glm-5.3-flash | https://opencode.ai/zen/go/v1/chat/completions" (OG). oboete's `kind = "openai"` client can call it.
  - Privacy: "GLM-5.3-Flash | Not used | 0 days" (OG; model training, data retention).
- MiniMax and Qwen on Go use the Anthropic endpoint, which oboete cannot call: "MiniMax M3 | minimax-m3 | https://opencode.ai/zen/go/v1/messages" (OG).
- Usage policy (OG, "Where can I use it?"): "OpenCode Go is designed for OpenCode and other coding agents that produce similar types of requests. Traffic is monitored for abuse that degrades the experience for other users." Clients should "Send typical coding agent traffic" and "Send a stable session ID in x-opencode-session for each conversation so we can optimize routing and prompt caching." The header is an expectation, not a hard requirement. The ZCode row on the same page says: "Our request for x-opencode-session remains open, but it is no longer necessary to send that specific header."
- oboete already names itself: `.user_agent(concat!("oboete/", env!("CARGO_PKG_VERSION")));` (src/provider.rs:222). It cannot send a header: the provider fields are name, base_url, key_file, model, daily_budget, timeout_s, retry_429 and extra (src/config.rs:75-92), and `extra` only edits the body: `body[k] = v.clone();` (src/provider.rs:215-217).
- Terms (TOS). The use rules forbid use that "automatically or programmatically extracts data or Output (defined below);" and "any processes that run or are activated while you are not logged into the Services". Read in context, the second sits in an anti-spam and load clause, and the FAQ lines above allow any agent. Also: "You will only use the Services for your own internal use, and not on behalf of or for the benefit of any third party". So each public user needs their own key.
- Privacy policy (PP): prompts are used for "Passing through to upstream provider to provide services"; the next cell says "Not stored".
- No tools reach the model. oboete's body holds only `model`, `messages`, `temperature` and `response_format` (src/provider.rs:209-214, checked 2026-09-26 while writing). I read the published gateway's live path at the pinned commit while writing (2026-09-26): it requires matching formats (`if (providerInfo.format !== opts.format) throw new Error("Zen provider format must match request format")`, handler.ts#L215), then forwards the request bytes (`return prepared.stream(providerInfo.model, providerInfo.format === "oa-compat")`, handler.ts#L222), replacing only the model value (`const initial = replace(chunks, found.start, found.end, providerModel)`, OC/packages/console/app/src/routes/zen/util/requestBody.ts#L99). For this format it appends only `,"stream_options":{"include_usage":true}}` (requestBody.ts#L201). New `oc_sk_` keys go another way (§5).
- This meets section 6: "a curator is a model call, not an agent. Prefer paths with no agent" (RD/section-6-revised.md:93). No subprocess runs. The key is read in-process and sent only as the `Authorization` header (src/provider.rs:229-231).

**How oboete would handle Go's errors today.** These error shapes are from the published gateway path (§5 for new keys).
- Limits come back as 429 with `retry-after` in seconds: `headers.set("retry-after", String(error.retryAfter))` (handler.ts#L513). oboete waits only up to `const MAX_WAIT_S: f64 = 60.0;` (src/provider.rs:20), so an hours-long Go reset falls through to the next provider.
- Auth, credit, monthly-limit and model errors come back as `{ status: 401 },` (handler.ts#L502). oboete treats 401 as an outage: `Some(401) => Some(COOLDOWN_OUTAGE),` (src/provider.rs:165). A typo in the model id looks like a 10-minute outage.
- A wrong-shape answer is not caught in the chain. Any parseable JSON is recorded as success: `db::record_call(conn, &name, "ok", ms, None)?;` (src/provider.rs:127). The window then fails later, without trying the next provider: `.ok_or_else(|| anyhow!("invalid output: observations is not an array"))?;` (src/observe.rs:225-227). If `observations` is an array of wrong-shape items, they are skipped silently and the window "succeeds" with zero observations (src/observe.rs:232-234).

**Owner scope.**
- Decision 21: "Subscription CLIs in public setup: explicit opt-in, off by default" (RD/owner-decisions.md:30). It says CLIs. Applying it to Go, an API-key subscription, needs the owner's yes.
- Decision 5: "paid APIs at most USD 5 per month" (RD/owner-decisions.md:9), as the owner's own default. Go costs $10 a month, so it only fits if treated as a subscription.
- Volume: the owner's `provider_calls` show 11 provider calls on 2026-09-23 and 13 on 2026-09-24 (local dates, embeddings excluded; checked 2026-09-26 while writing). Go's allowance is far larger. The $10 mostly makes sense if the owner also codes with Go.
- Issue #55 lists "未承認provider・有料経路の追加" under "対象外" (https://github.com/ojungo69/oboete/issues/55). So no PR adds Go before the owner says yes.

### 3.4 Before Go goes in

Code changes (none of them is Go-only):
1. **Check the answer's shape inside the chain.** A mismatch becomes `invalid output` and the chain moves to the next provider. This is needed for every provider without proven strict-schema support. Issue #54 already has the acceptance test: "中間チャンクのprovider失敗／schema失敗／DB保存失敗" (https://github.com/ojungo69/oboete/issues/54).
2. **Per-provider request headers**, for `x-opencode-session` with a stable id per curation session.
3. **The subscription mark on `kind = "openai"` entries.** Section 1 already speaks of "providers marked subscription" (RD/sections-1-4.md:12). The config has no such field yet (src/config.rs:75-92). With it, the wait-while-working gate and the opt-in setup treat Go like the subscription CLIs.

Owner actions, if yes: create the key and save it as `/home/jura/OPENCODE_API_KEY.md` with the token on line 2. `key_file` is read as given, with no `~` expansion: `let key = text.lines().nth(1).map(str::trim).unwrap_or("");` (src/config.rs:284). No such file exists yet (`ls ~/*_KEY.md` lists only CF_WORKERS_AI_KEY.md, GROQ_API_KEY.md, MISTRAL_API_KEY.md, NVIDIA_NIM_KEY.md and `/home/jura/OPENROUTER_API_KEY.md`). In the console, keep "Use balance" off and Zen auto-reload off.

**The config.toml entry** (only after the owner's yes and change 1):

```toml
# OpenCode Go: subscription, explicit opt-in. Console: "Use balance" OFF, Zen auto-reload OFF.
[[providers]]
kind = "openai"
name = "opencode-go"
base_url = "https://opencode.ai/zen/go/v1"
key_file = "/home/jura/OPENCODE_API_KEY.md"   # absolute path; token on line 2
model = "glm-5.3-flash"                       # $0.15/$0.50 per 1M, $60/month, not used for training, 0 days
daily_budget = 300
retry_429 = true                              # an hours-long Go 429 falls through anyway (MAX_WAIT_S = 60)
```

- Place it after the free APIs until one probe with synthetic text shows it keeps the schema. Then it can move up.
- Any `[[providers]]` replaces the whole default chain: `#[serde(default = "default_providers")]` (src/config.rs:10-11). The owner's `~/.oboete/config.toml` has no `[[providers]]` line today (checked 2026-09-26 while writing). So adding Go means writing out the full chain. It must not list grok (decision 23) and should not re-add agy (PR #51).
- Judge and digest use the same entry shape: "Each role (curator, judge, digest) can use its own chain" (RD/sections-1-4.md:12).

### 3.5 The opencode CLI as a curator: no

The researcher found no proven no-tool mode and no schema flag in `opencode run --help` (v2.0.12). Section 6 skips such a CLI for every role that reads recorded text. The Go API path does the same job with no agent.

## 4. Changes to the design spec

Quoted fragments are the current text.

**Section 1 (RD/sections-1-4.md)**
- :12 "any of claude, codex, agy, grok, each with its model and daily cap" → drop grok (decision 23). Add "and API-key subscriptions (OpenCode Go, opt-in)". Keep "which free APIs (Groq, OpenRouter free, NIM, Mistral)" as is: OpenCode Zen free models are not offered (403 gate). Keep "The wait-while-working gate applies to providers marked subscription." and add that a `kind = "openai"` entry can carry the mark.
- :31 "Failed provider: next in chain." → "Failed provider, including an answer whose shape does not match the schema: next in chain." The "cooldown" reason can carry a reset time from claude's `rate_limit_event`, kept across runs.
- :52 "the gate applies to subscription CLIs only, because they share the quota the owner uses while coding" → "applies to providers marked subscription (the CLIs, and OpenCode Go if adopted), because each may share an allowance the owner uses while coding".

**Section 6 (RD/section-6-revised.md)**
- :61 and :63 (grok's kept copies as a curator) → grok no longer curates. Remove it from "Copies kept by the curator CLIs". Its own transcripts stay under the recorded-agent bullet at :54.
- :76 (S8 allow-list) → add: "No `ANTHROPIC_*` or `CLAUDE_CODE_*` name is allow-listed." (Whether cloud-provider selection variables should pass is open, §5.)
- :91 (claude-mem) → correct "locked in layers ... a `canUseTool` callback that denies every call and writes an audit entry ... Its note: on the CLI path only the deny list applies". Every layer except `canUseTool` maps to a CLI flag. On the streaming Observer spawn, `--tools ""` is removed by claude-mem's spawn factory. Under `dontAsk`, `canUseTool` never runs. The deny list names `Task` but not `Agent`, and not `Monitor`. These are read from code, not run. Also note that the file is unchanged at 02cd0c9.
- :93 "claude gains claude-mem's extra layers where the CLI has them (`--permission-mode dontAsk`, an explicit `--disallowedTools` list) and a log line for any tool attempt seen in the output" → add `--permission-prompts none`, `mcp__*` in the deny list, `--disable-slash-commands` and `--system-prompt-file`. The deny list names both `Agent` and `Task`, and `Monitor`.
- :97 (per-call tool check) → for claude, also check MCP servers, plugins, the permission mode and `apiKeySource`. Read `rate_limit_event` from the same stream (persisted cooldown until `resetsAt`; `credits_required` stops claude until the owner acts).
- :100 "claude: ... kept." → "kept, plus §2.3 items 1-3 of docs/research/curator-providers-2026-09-25.md" (or list them).
- :101-103 (grok `--tools ""`, `--disable-web-search`, `--no-subagents`, `--sandbox`) → replace with "grok: not used for curator, judge or digest (decision 23)". :170 (finding H, grok network) → mark superseded by decision 23.
- :143 "all four CLIs still authenticate on all three OSes" → "every CLI used as a curator still authenticates on all three OSes".
- New bullet under §5: "OpenCode Go (if adopted) is an API call with no tools in the request and the key in-process. It needs no isolation gate beyond the shape check."

**Section 7 (RD/section-7-revised.md)**
- :54 (tier line) → quote CC/legal-and-compliance lines 50, 52 and 43 as in §2.5, checked 2026-09-26. "The same applies to codex, grok and agy" → "codex and agy" (decision 23). Add: "OpenCode Go, if adopted, is also an opt-in subscription. Its line quotes Go's usage policy and the terms' own-internal-use clause, and says prompts pass through to the model's provider." OpenCode Zen free models are not offered.
- :193 "provider budget left;" → add "and any stored cooldown with its reset time (claude's `resetsAt`; `credits_required` shown as needing the owner)".
- :197 "curator isolation status;" → add "claude's last init check (tools, MCP servers, plugins, permission mode, `apiKeySource`) and whether the installed CLI accepts `--permission-prompts none` (v2.1.259 or later)". An older CLI would reject the unknown flag, so doctor probes it with `--version` before use.
- :230 "what leaves the machine for each tier" → for Go, list each chosen model's training and retention row.
- :268 "Public setup therefore does not offer subscription CLIs (owner question 2)." → superseded by decision 21 (option b).
- :285 "(the audit's third quote, about "the unmodified Claude Code binary", is not on that page and is withdrawn)" and :292 "the current page no longer has that sentence" → reverse. On 2026-09-26 the page has it at line 52 (§2.5). Also :288's quote now reads "to offer Claude.ai login into their own applications, or to route requests". Either the page changed after 2026-09-25 or the earlier read missed it; this note does not decide which. :291 "does not clearly bless either" → line 52 now covers the owner's path. This strengthens option (b); decision 21 stands.
- :295 "the terms for codex, grok and agy" → "codex and agy (and OpenCode Go if adopted)".

## 5. Unverified and open questions

**Claims that failed verification** (not used as facts above):
- *"In `-p` mode the API key is always first."* The sentence is at CC/authentication line 205, but it is item 3 of the list. Cloud-provider credentials and `ANTHROPIC_AUTH_TOKEN` rank above it, per the verifier. It only means `-p` skips the approval prompt for a key. This note relies only on `/login` being last.
- *"Every CLI failure gets oboete's 600 s cooldown."* False for invalid output: `cooldown_for` returns no cooldown when the message starts with "invalid output" (src/provider.rs:157-170, per the verifier). Only the other status-less CLI failures get 600 s.
- *"Space Bunny Free is the only free model with zero retention and no training."* The page's exception list does not name Jev 1.13 Free, so it too falls under the zero-retention default. The verifier's reading: Space Bunny Free is the only free model on `/chat/completions` that is not a privacy exception.
- *"Free models always get the per-IP limiter, even with a key."* True only for old `sk-` keys that were not migrated. New keys are proxied before the limiter. So "the 200/day quota is shared with the owner's own OpenCode use" holds for callers with no key behind the same IP.
- *"The gateway passes `response_format` through / builds tools only from the request"*, as cited (OC openai-compatible.ts#L219, #L198). Those lines are in a converter the live path does not use. §3.3 cites the live path instead.
- *"New `oc_sk_` keys go to a service whose source is not published."* The routing is real: "// New Console keys are never in the legacy key table; every legacy key is `sk-`." (OC/packages/console/app/src/lib/inference-proxy.ts#L46), and `const legacy = !key.startsWith("oc_sk_")` (#L47) is checked before the legacy handler's model, auth and balance checks (handler.ts#L107-L112). But the destination's code was only not found in the repo at 34aa427; it is not proven unpublished. Consequence: every gateway behaviour in §3 (limit errors, 401s, "Use balance", the key limit of `const LIMIT = rateLimit ?? 1000` in keyRateLimiter.ts#L15) is proven for the old path only. A key created now is likely a new Console key (`oc_sk_`), so these may not apply to Go.

**To confirm in one dogfood call** (in the `oboete-dogfood` user, synthetic text):
1. Whether `claude -p --output-format stream-json` emits `rate_limit_event` on every request or only near a limit.
2. Whether `system/init` arrives before the CLI reads stdin. If so, oboete can hold the prompt until the tool check passes.
3. Whether `--json-schema` adds a structured-output tool to `init.tools`, and whether the final `result` event carries `structured_output` in stream mode.

**Other unknowns.**
- How subscription usage weights cache writes and reads. The 8,400-token estimate in area 3 assumes API weighting.
- Whether `--setting-sources ""` also stops user plugins, skills and CLAUDE.md. The init check settles it on every call.
- How `claude -p` exits at a usage limit (exit code, message), and whether a rejected request uses quota.
- Whether `Task` is still an alias for `Agent` in deny rules. The deny list names both anyway.
- `--effort low` for the curator. Some models may reject it; test first.
- Whether the allow-list should pass `CLAUDE_CODE_USE_BEDROCK/VERTEX/FOUNDRY` (a user who runs Claude through a cloud provider) or drop them (keeps the curator on the subscription). On Windows, whether `claude` needs `CLAUDE_CODE_GIT_BASH_PATH` (claude-mem preserves it, per the researcher; not verified).
- claude-mem's lockdown holes (area 5) are read from code, not run.
- Whether `glm-5.3-flash` on Go enforces a strict `json_schema` and accepts `temperature: 0.2`. No key exists, so nothing on Go was measured.
- The limits and error bodies for new `oc_sk_` keys. Check the first real 429 body against `retry-after`.
- The enforced free-tier number (the 200/day line is FAQ copy; the limiter's own value was not checked).
- The probe files keep only response headers, so "no key was sent" rests on the researcher's record.

**Owner decisions.**
1. Claude: how much of the 5-hour and weekly allowance may background curation use? First step reacts only to `rejected`. claude-mem stops at, for example, `five_hour: 0.95,`. Options: stop at `allowed_warning`, or a utilization cap.
2. Claude: keep Sonnet or move to Haiku 4.5 like claude-mem? It changes quality and usage. No source here compares quality.
3. Go: is a background curator acceptable under "Send typical coding agent traffic" and the terms quoted in §3.3? The alternative is to ask OpenCode first.
4. Go: pay $10 a month for about a dozen curator calls a day, above the owner's USD 5 paid-API default? It makes sense mainly if the owner also codes with Go.
5. Go: does decision 21's opt-in for "Subscription CLIs" also cover an API-key subscription like Go?
