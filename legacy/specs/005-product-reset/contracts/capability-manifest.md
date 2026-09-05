# Contract: Effective Capability Manifest

## Purpose

Make setup, runtime, and doctor agree on one effective non-secret configuration.

## Inputs

- the fixed Slice 1 `ResourceProfileV1`
- one closed `ProviderProposalV1` containing only `version: 1`, `role: summary`, `state: enabled`,
  model identity, `anthropic_messages_v1 | openai_chat_completions_v1`, one complete canonical
  `endpointUrl`, and `credentialRef: {kind:none} | {kind:environment,name}`
- an explicitly disabled embedding provider with a machine-readable reason
- detected platform and Agent capabilities
- supported InjectionPack destination-policy map keyed by Agent/model destination class, with
  execution location and egress policy
- compatible legacy key dispositions, when migration is requested

Provider proposals are closed. Provider names/kinds, runtime-appended paths, split scheme/host,
inline or free-form credentials, arbitrary headers/cookies, and self-declared execution, egress,
cost, TLS, redirect, or fingerprint fields are rejected.

## Output

The compiler returns either:

- a validated immutable manifest ready for atomic activation, or
- structured validation failures that leave the prior manifest active.

The compiler preserves the proposal fields and derives `executionLocation`, `egressPolicy`,
`costClass`, `tlsPolicy`, and literal `redirectPolicy: reject`. It computes
`providerFingerprint = SHA-256("free-mem:provider-choice:v1\0" || JCS(choice without
providerFingerprint))`. The effective manifest includes that closed choice, the profile,
destination map, disabled embedding lane, sorted legacy dispositions, predecessor binding when it is
a successor, and a computed non-secret configuration fingerprint.

## Invariants

- Summary and embedding states are independent. Disabled is explicit, never an empty model or
  endpoint sentinel.
- `new URL(endpointUrl).href` must equal the supplied URL. Userinfo, query, fragment, an empty/root
  path, and unsupported schemes are rejected. Literal `127.0.0.1` and `[::1]` alone are local;
  `localhost`, localhost subdomains, trailing-dot hostnames, wildcard/unspecified addresses, and
  alternate loopback spellings are rejected. Local endpoints may use HTTP only with credential
  `none` and eligible-only projection, or HTTPS with verified chain/hostname peer identity. Every
  remote endpoint is HTTPS with system chain and hostname validation.
- Provider egress policy distinguishes on-device consumers from remote/off-host destinations;
  private and local-only data are eligible only for the former and secrets are eligible for neither.
- Technical Alpha provider HTTP redirects are rejected before any payload is resent, and doctor
  reports the bounded redirect reason. The rejected job resumes only after activation of a changed,
  validated configuration for that provider; the prior `Location` is never followed or replayed.
  A later redirect allowlist requires a new explicit contract.
- Every remote/off-host ProviderChoice has `executionLocation=remote`,
  `egressPolicy=explicit_remote`, `costClass=external_metered`, and `tlsPolicy=system`. Literal
  loopback choices derive `local`, `on_device`, `local_zero`, and `not_applicable` only for HTTP.
  That HTTP form never authorizes private/local-only payload or credential bytes. Verified local
  HTTPS derives `executionLocation=local`, `egressPolicy=on_device`, `costClass=local_zero`, and
  `tlsPolicy=system`, and may process restricted same-repository content.
  Insecure TLS bypasses, initial remote HTTP, and HTTPS-to-HTTP downgrade are rejected before
  credentials or payload bytes are sent and are reported by setup/doctor.
- Each InjectionPack request supplies its concrete target Agent/model destination and resolves it
  against the active manifest's policy map. Unknown destinations are remote/ineligible for private
  and local-only data; either disposition requires a matching explicit on-device policy.
- Claude Code, Codex, and MCP resolve remote/unknown in Slice 1 production. Local destination
  classes are runner-only and require candidate-inaccessible loopback-consumer evidence; a local
  client process, Agent/model label, or caller field grants no local authority.
- Secret values never appear in the manifest, logs, doctor output, or fingerprint.
- Runtime consumers do not read legacy configuration or provider environment independently.
- Manifest fingerprint input is the complete manifest after provider fingerprinting, excluding only
  `configurationFingerprint`, with domain `free-mem:effective-capability-manifest:v1\0`.
- The fixed base remote choice is `openai_chat_completions_v1` at
  `https://summary.stub.invalid/v1/chat/completions` with environment credential
  `FREE_MEM_SUMMARY_API_KEY`. The complete local successor uses
  `https://127.0.0.1:1234/v1/chat/completions` and credential `none`. One complete repaired-remote
  successor uses `https://summary-repaired.stub.invalid/v1/chat/completions`. Configuration,
  redirect, and downgrade recovery signals bind the successor's computed provider and manifest
  fingerprints; scenario-local provider overrides and summary-config labels are not accepted.
- Base, local, and repaired manifests use profile version 1 with derivation limit 16. The only
  output-limit recovery is a complete runner-owned test successor at version 2/limit 17; production
  exposes no selectable profile override.
- `ResourceProfileV1` also fixes source events/job 100, observer request timeout 60,000 ms, maximum
  input 12,000 JavaScript UTF-16 units, output 4,000 tokens, response 1,048,576 bytes, temperature 0.2, provider
  TLS preflight timeout 5,000 ms, periodic sweep 30,000 ms, idle flush 120,000 ms, debounce 1,000 ms,
  stuck claim 300,000 ms, and raw-event retention disabled/0 ms.
- After setup confirmation and again at daemon start, every HTTPS choice performs a native
  credential-free, payload-free TLS handshake to the exact host/port/SNI with normal chain and
  hostname validation within 5,000 ms. Setup failure mutates nothing or restores the prior
  activation. Daemon-start failure preserves writer/RPC/capture/spool-import/lexical services and
  disables only provider/AI processing as `provider_unavailable` or `provider_tls_rejected`.
  Production rejects added CA path/environment configuration; only platform system trust is used.
  The isolated runner installs its public test CA into its private system trust before candidate
  start, outside proposal/manifest/candidate control. Local HTTP skips this preflight and remains
  credential-none/eligible-only.
- Doctor reports the active manifest, not a separately reconstructed approximation.
- Active `legacyDispositions` contain at most 64 sorted unique closed keys and only `translated`,
  `ignored`, or `overridden`, with no values. A conflict rejects compilation and is not representable
  in an active manifest.
- Model or index changes that require rebuilding never remove lexical retrieval during transition.

The deterministic summary stub remains runner pins/scenario metadata and materializes an ordinary
proposal. It is not a production provider kind or registry entry. Runner-observed provider cost is
zero because that stub is runner-owned; the remote ProviderChoice remains `external_metered`.

The protocol names freeze transport behavior. Anthropic Messages sends JSON content type,
`anthropic-version: 2023-06-01`, and `x-api-key` only for an environment credential; its request is
`{model,max_tokens,temperature,system,messages:[{role:"user",content}]}` and response text is the
ordered concatenation of `content[]` text blocks. OpenAI Chat Completions sends JSON content type and
`authorization: Bearer` only for an environment credential; its request is
`{model,max_tokens,temperature,messages:[{role:"system",content},{role:"user",content}]}` and
response text is `choices[0].message.content`. Credential `none` emits no authentication header.
Responses are byte-bounded before JSON parsing; streaming, Responses API, tools, custom headers,
tier routing, and fallback are unsupported.

Input length is JavaScript UTF-16 code units. Reserve a 3,000-unit user floor: slice system from the
start to 9,000 units and call `toWellFormed()`, then slice user from the start to
`max(3,000, 12,000 - clippedSystem.length)` units and call `toWellFormed()`. Both protocols use this
same allocation; there is no tail merge, token-based alternative, or provider-specific reallocation.
