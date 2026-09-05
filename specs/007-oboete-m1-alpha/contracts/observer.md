# Observer Contract (M1)

One JSON schema is produced by the LLM observer and by the rule-based fallback; the worker treats
both identically except for `degraded_reason`.

## Batch composition and the outbound boundary

After classification, a session batch is split by destination; batch identity is
(session, through event, destination). Every batch produces observations only. The session
summary is never a provider output in M1: at session end the worker derives it deterministically
from the session's rows and the observations just applied (rules below), so session end costs
exactly one provider call (the observations batch) and the summary has one source. The matrix:

M1 enables exactly one observer preset at a time (the spec speaks of "the configured provider
preset"); the matrix:

| configuration | session end provider calls | observations from | summary from |
|---|---|---|---|
| remote preset | 1 (remote batch, eligible rows) | remote batch + fallback batch for the remaining rows | deterministic summary |
| local preset | 1 (local batch, non-secret rows) | local batch | deterministic summary |
| no preset / degraded | 0 | fallback batch | deterministic summary |

Rows are assigned to exactly one destination, so no observation is generated twice (tested with
a remote preset by asserting that the fallback batch of the same range contains no eligible row):

| destination | receives | when |
|---|---|---|
| `remote_observer` | `eligible` rows only | a remote preset is configured and consented |
| `local_observer` | `eligible`, `local_only`, `private` rows of the same repository | a local preset (Ollama) is configured |
| `fallback` | rows no other destination took | always |

`secret` rows are never summarized. **One request builder** (`observer/request.ts`,
security-owned) assembles every outbound request and applies `destination_rules` to every field:
`events`, `free_summaries`, `nearby` (a remote request contains only `eligible` memories),
`citations`, and repository metadata (a remote request carries an opaque `repo_ref` = the repository
id, never the normalized remote or path). Tests assert the actual request body of a mixed batch.

The producing agent is provenance only and never appears in the request. Each memory records its source rows in `memory_sources` and takes the strictest sensitivity of
those rows.

## Input (worker → summarizer)

```json
{
  "repo_ref": "<repository id>",
  "session": { "started_at": 0, "turns": [ ... ] },
  "events": [ { "id": "e1", "kind": "prompt", "text": "..." }, { "id": "e2", "kind": "tool_call", "tool_name": "edit", "input": { ... } } ],
  "free_summaries": { "last_assistant_message": "...", "compaction_summary": "..." },
  "nearby": [ { "id": "m1", "type": "decision", "title": "...", "body": "...", "deleted": false } ],
  "language_hint": "ja"
}
```

Total input is excerpted to 12,000 characters (FR-015): free summaries first, then prompts, then
tool inputs and outputs by recency; `observation_batches.excerpted` records it.

## Output (both paths)

```json
{
  "observations": [
    {
      "type": "bugfix | feature | refactor | change | discovery | decision | security_alert | security_note",
      "title": "<= 120 chars, language of the content",
      "body": "<= 2000 chars",
      "concepts": ["how-it-works", "why-it-exists", "what-changed", "problem-solution", "gotcha", "pattern", "trade-off"],
      "citations": { "files_read": [], "files_modified": [], "commits": [] },
      "source_event_ids": ["e1", "e2"],
      "classification": { "decision": "add | update | delete | noop", "target": "<nearby id or null>", "reason": "<short reason>" }
    }
  ]
}
```

`source_event_ids` is required on every observation and must be a non-empty subset of the `id`
values supplied in `events` (same batch, same repository); an observation citing an unknown,
empty, or foreign id is rejected as `unusable_output` (one retry). The fallback fills it by rule
(the events of the turn for `change`, the failed and the retried call for `bugfix`, the failed
call for `discovery`, the message event for `decision`). **Output budget and schema caps** (shared zod schema, both paths): at most 20 observations
per batch, `source_event_ids` at most 50, each citation string at most 512 characters, at most 20
paths and 10 commits per observation; the HTTP response body is capped at 1 MB (larger →
`unusable_output`), which bounds worker memory. Every title is trimmed to 120 characters and every
body to 2,000 characters by a deterministic order (display paths shortened to their last 60
characters, lists cut from the end, an `... (+N omitted)` suffix recording the omission); the
full citation path is kept in `memory_sources.citation_value` for the staleness check, the
shortened form appears only in bodies.

**Session summary (deterministic, worker-side, no provider call)**: `type = session_summary`,
`title` = the session's first prompt truncated to 120 characters, `body` = five labelled lines
under the same 2,000-character budget: `request` = the first prompt (up to 1,000 characters);
`investigated` = the distinct files read (up to 20 paths); `learned` = the titles of the
observations applied for the session (up to 10); `completed` = the distinct files modified with
tool counts (up to 20); `next_steps` = the last unfinished turn's prompt (200). Trim order: the
three list lines drop entries from the end until five remain in each, then `request` gives back
characters down to a floor of 200, and only a body still over budget empties the lists further, so
a long prompt keeps the developer's exact words without evicting the session's findings (A20).
Sensitivity =
strictest source row. `degraded_reason` = the most severe reason among the session's batches by this fixed precedence:
`provider_paid` > `provider_exhausted` > `auth_failed` > `consent_changed` > `daily_cap` >
`unreachable` > `timeout` > `unusable_output` > `language_mismatch` > `model_alias` >
`no_provider` > `rule_based` (NULL only when every batch was applied from a provider), so a
no-credentials session yields a summary labelled `no_provider` and the session-start pack shows
`Degraded:` (SC-004). **Durable
completion**: an `ended` session with `summary_state = pending` whose batches are all terminal
is reconciled by every worker run; the summary insert, `latest_summary_memory_id`, and
`summary_state = done` commit in one fenced transaction, so a crash between the last batch and
the summary cannot leave the session without one. A session with zero summarizable events (no
prompt, tool, assistant-message, or compaction row with non-empty non-secret content after
`<private>` removal; lifecycle rows never count) is set to `summary_state = no_content` with
**no memory row and no injection** (spec edge case "nothing is produced, nothing is sent"); a
fixture with `session_start`, a fully `<private>` prompt, and `session_end` asserts the absence
in the database and in the next session-start pack and that reconciliation does not revisit it. SC-004 (three
seeded facts recalled with no credentials) is asserted against the fallback observations plus
this summary.

Worker rules after either path: the detector runs again on every title and body; the
directive-corpus check rejects bodies that read as instructions; sensitivity on `add` = strictest
source row and detector result, on `update` = max(target's sensitivity, every source row, detector
result), fixed in the apply transaction so a `local_only` or `private` target can never be
relaxed by an eligible update (tested against the outbound body); `material_hash` and `content_hash` come from the shared identity helper (`material_hash` =
sha256(normalized title, normalized body) (observation type excluded, A13), `content_hash` = sha256(repo_id, material_hash);
the same function serves import and tombstones); `target` must be
one of the supplied `nearby` ids from the same repository, otherwise the decision is `add`; a hash
matching a tombstoned memory suppresses the insert and is recorded for `why`; `update` sets
`valid_to` and `superseded_by`; `delete` tombstones the target only with a non-empty `reason`; the
observer answers in the dominant language of the input (FR-014); the worker compares the
dominant script of every title and body with the input's, retries once on mismatch, and on a
second mismatch discards the output and routes the batch to the fallback with
`language_mismatch` (fallback records copy input text verbatim, so their language is the
input's); a provider fixture returning English for Japanese input verifies this.

## Provider presets

| preset | package | endpoint | credential | cost class | structured output |
|---|---|---|---|---|---|
| `workers-ai` (default) | `workers-ai-provider` REST | `.../accounts/<id>/ai/run/@cf/zai-org/glm-4.7-flash` | `OBOETE_CF_API_TOKEN` + `OBOETE_CF_ACCOUNT_ID` | free tier, ~45 neurons per call | JSON schema (verified live) |
| `ollama` | `@ai-sdk/openai-compatible` | `http://127.0.0.1:11434/v1` | none | local | `response_format` |
| `nim`, `openrouter`, `gemini` | `@ai-sdk/openai-compatible` | provider base URL | `OBOETE_<PRESET>_API_KEY` (`OBOETE_NIM_API_KEY`, `OBOETE_OPENROUTER_API_KEY`, `OBOETE_GEMINI_API_KEY`) | remote | `response_format` where the R13 probe confirms it, else text-JSON |
| `agent-cli` (optional) | child process: `claude -p --output-format json`, `codex exec --json`, or `grok -p --output-format json` | none (the CLI's own login) | own subscription, shown on the consent screen | text-JSON; exempt from the 150-call cap; headless JSON output per CLI is an R13 probe and a failure only disables this preset |

Text-JSON path: the prompt asks for exactly one JSON object; the reply is parsed and validated with
the same zod schema; failure counts as `unusable_output`.

## Call policy

1. **Per attempt**: in one short `BEGIN IMMEDIATE` transaction, check the daily cap (FR-012: 150
   HTTP attempts per UTC day summed over all capped presets; attempt 150 allowed, 151 refused;
   when 10 or fewer remain, `ten_turns` batches go to the fallback and the remainder is reserved
   for `session_end` batches; `agent-cli` is not counted) and the preset's `exhausted_at`; if either blocks, mark the batch degraded
   (`daily_cap` or `provider_exhausted`) and route it to the fallback; otherwise increment
   `provider_usage.calls` and `observation_batches.provider_attempts`, record a reservation id,
   set the batch `running`; commit. A retry is a new attempt and a new reservation.
2. Network call outside any transaction, `maxRetries: 0`, `abortSignal: AbortSignal.timeout(60_000)`.
3. Classify by status **and** provider body code (table-driven, tested with the same status and
   different codes): 429 + 3036 → `provider_exhausted`; 403 + 5035 → `provider_paid`; 401, or 403
   without 5035, or any auth/permission body code → `auth_failed` (recovery: check the credential
   variable); 408/3007 or 429/3040 → one retry; `length`, null content, invalid JSON → one retry
   then `unusable_output`; abort → `timeout`; returned model id mismatch → `model_alias`; network
   error → `unreachable`.
4. **Exhaustion persistence**: a 3036 result writes `provider_usage.exhausted_at` (idempotent,
   monotonic, keyed by the reservation id) in its own transaction that is **not** fenced by the
   lease, so the signal survives a lost lease.
5. Apply: memory mutations (add, update, delete) and `state = applied` are written in **one**
   transaction fenced by `owner_token`; zero rows changed means the lease was lost and the whole
   result is discarded (the exhaustion signal from step 4 is not). A worker that dies after the
   response and before this transaction leaves the batch `running`; the next worker reclaims it
   after 120 s and makes a new call (at-least-once attempts, exactly-once applied effects, A11).
6. **Consent**: the consent tuple hash is recomputed from live configuration before step 1 and
   again immediately before step 2; a mismatch makes no call and degrades with `consent_changed`.
7. Neurons: `cf-ai-neurons` header when exposed, else tokens × (5,500 / 36,400 per million).

## Rule-based fallback

Deterministic, no network, copies input text verbatim (so the language is the input's), emits
records rather than prose, at most 20 observations per batch, every field of the schema filled by
rule:

| record | when | title | body | concepts | citations | classification |
|---|---|---|---|---|---|---|
| `change` | per turn with file modifications | the modified paths joined by `, ` (<= 120 chars) | one line per tool call: `<tool> <path> (+<added>/-<removed>)`, at most 40 lines then `... (+N omitted)` | `what-changed` | `files_modified` = the paths (<= 20); `commits` = commit ids seen in tool output (<= 10) | see below |
| `bugfix` | a `tool_failure` followed by a successful call of the same tool in the same turn | `<tool>: <first error line truncated to 80>` | first error line (200) + the successful call's first line (200) | `problem-solution` | `files_modified` of the retry | see below |
| `discovery` | a `tool_failure` with no successful retry | same as `bugfix` | first error line (200) | `gotcha` | `files_read` of the failed call | see below |
| `decision` | per `last_assistant_message` or `compaction_summary` | first sentence (120) | first paragraph (2,000), verbatim | `why-it-exists` | none | see below |

Fact retention for SC-004: the three seeded facts appear in prompts and tool outputs; the
`decision` record keeps the first paragraph of `last_assistant_message` verbatim and the
`change` record keeps every modified path, which is where the fixture plants them.
`classification.reason` = `rule:<record>`; decision = exact `content_hash` match on a tombstoned
row → suppressed, on an active row → `noop`, otherwise `add`; the fallback never emits `update`
or `delete`. `degraded_reason` is set by the worker: a provider failure reason (`no_provider`,
`unreachable`, ...) when the fallback replaced a failed provider call, `rule_based` when the rows
went to the fallback by design (local-only rows next to a healthy remote preset); NULL is reserved
for provider output.
