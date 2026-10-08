# Anthropic's Messages API as a curator entry (2026-10-09)

The owner, 2026-10-09: 「anthropicが毎月無料クレジットを配ってくれるので候補に入る」 (Anthropic gives the
account monthly credits, so it belongs among the candidates). This note records what the entry
(`api = "anthropic"` on a `kind = "openai"` entry, and `anthropic` in the default chain) rests on.
Every fact below was read on 2026-10-09 from Anthropic's own pages, named with each. No call was
made to the API.

## Why the Messages API

Anthropic's OpenAI-compatible endpoint ignores `response_format` and `strict`, and its page says it
"is not considered a long-term or production-ready solution for most use cases"
(https://platform.claude.com/docs/en/cli-sdks-libraries/libraries/openai-sdk, where
https://platform.claude.com/docs/en/api/openai-sdk leads). oboete needs JSON that follows the schema,
so an entry speaks the native Messages API instead.

## The request

Structured outputs (https://platform.claude.com/docs/en/build-with-claude/structured-outputs):

- `POST {base_url}/messages` (base URL `https://api.anthropic.com/v1`), headers `x-api-key`,
  `anthropic-version: 2023-06-01`, `content-type: application/json`. No beta header:
  `output_config.format` is generally available; the older `output_format` is deprecated and needs
  the `structured-outputs-2025-11-13` beta header.
- Body: `model`, `max_tokens` (required), `messages: [{"role": "user", "content": ...}]`,
  `output_config: {"format": {"type": "json_schema", "schema": ...}}`.
- The JSON is the text of the answer's text block. A thinking block can come before it, so the
  first block of type `text` is read. oboete unfences it as it unfences an OpenAI answer.
- `stop_reason: "refusal"`: HTTP 200, billed, and the output may not match the schema.
  `stop_reason: "max_tokens"`: the output may be incomplete. oboete counts both as failed answers,
  with the tokens they billed.
- Models with structured outputs include `claude-haiku-5-5` and `claude-haiku-4-5-20251001`.
- Schema limits: `additionalProperties` must be `false` on objects. Refused with HTTP 400:
  recursive schemas, complex types in enums, external `$ref`, numerical constraints (`minimum`,
  `maximum`, `multipleOf`), string constraints (`minLength`, `maxLength`), and array constraints
  beyond a `minItems` of 0 or 1. A request takes at most 24 optional parameters and 16 with union
  types. oboete's curator, summary and probe schemas use none of the refused keywords. The copy
  sent drops them anyway and sets `additionalProperties: false` on every object; the answer is
  still checked locally against the whole schema.

## The model and its parameters

Prices in USD per million tokens (https://platform.claude.com/docs/en/about-claude/pricing):

| Model | Input | Output |
|---|---|---|
| Claude Haiku 5.5, prompts up to 100,000 tokens | 0.10 | 0.50 |
| Claude Haiku 5.5, prompts over 100,000 tokens | 0.50 | 2.50 |
| Claude Haiku 4.5 | 1 | 5 |

The default entry is Haiku 5.5, the cheaper Haiku with structured outputs. It is priced at the
first row, with `max_request_tokens = 100000`, so no prompt reaches the second; a curation window
is about 6,000 tokens. Its prices put it inside `paid_usd_per_month` with every other paid entry.

The Haiku 5.5 migration guide (https://platform.claude.com/docs/en/models/haiku-5-5/migration-guide):

- A request that includes `temperature` must send `1`; any other value is a 400, as is any
  `top_k` and a `top_p` other than 0.99. So a Messages request sends no `temperature`, where an
  OpenAI-compatible request sends 0.2; an entry's `extra` can add one for a model that takes it.
- Adaptive thinking is on by default, and thinking tokens count toward `max_tokens`: a small
  `max_tokens` can stop after the thinking, before any text. `thinking: {"type": "disabled"}` is
  accepted at effort `low`, `medium` (the default) and `high`
  (https://platform.claude.com/docs/en/api/errors, "Thinking cannot be disabled"). The default
  entry turns thinking off in its `extra`, as nim's reasoning is turned off, so an answer is not
  cut short at its 4,000 tokens. The connection test keeps the entry's own `thinking`.

## Errors and limits

Errors (https://platform.claude.com/docs/en/api/errors): the body is
`{"type": "error", "error": {"type": ..., "message": ...}, "request_id": ...}`. 400
`invalid_request_error` (also when a spend limit the user set is reached), 401
`authentication_error`, 402 `billing_error`, 403 `permission_error`, 404 `not_found_error`, 413
`request_too_large`, 429 `rate_limit_error`, 500 `api_error`, 504 `timeout_error`, 529
`overloaded_error` (the API is temporarily overloaded).

- Spent credits: a 400 `invalid_request_error` whose message says the credit balance is too low.
  oboete reads only the error's own message for that, keeps the vetted code with "(credits
  spent)", and rests the entry until the next month (UTC), as a spent month's paid budget does. A
  402 `billing_error` rests it the same way. `oboete resume anthropic` ends the rest at once, after
  a top-up.
- 529 rests the entry as a 503 does (ten minutes).
- ureq drops `Authorization` on a redirect but keeps other headers, `x-api-key` among them, so a
  Messages entry follows no redirect.
- Rate limits (https://platform.claude.com/docs/en/api/rate-limits): a 429 carries `retry-after`
  in seconds, read as Groq's is. The `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`
  headers give resets as RFC 3339 times, and tokens remaining rounded to the nearest thousand; the
  `tokens` ones show the most restrictive limit in effect. oboete keeps `requests-remaining` and
  `-reset` and `tokens-remaining` and `-reset`, as it keeps Groq's `x-ratelimit-*`.
- A usage tier's monthly spend cap answers 429 with `error.details.error_code` =
  `enforced_spend_limit_reached` and no `retry-after`, until 00:00 UTC on the first of the next
  month. oboete's own cap of paid calls (USD 5 by default) keeps far below the smallest tier's
  (USD 500), so it is not handled apart.
