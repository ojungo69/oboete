# Contract: Effective Capability Manifest v1

## Contract-first prerequisite

The existing Product Reset fixture/schema/semantic validators and bound result examples under
`specs/005-product-reset/` predate this buildable provider/resource shape. They must receive one
mechanical contract correction and fingerprint recomputation, with their full validator suite green,
before runtime implementation. PR 0 co-delivers that correction with this 006 contract.

The deterministic provider stub remains runner metadata. The runner materializes an ordinary
ProviderProposalV1; production has no stub provider kind or generic provider registry.

Likewise, Claude Code, Codex, and MCP are remote/unknown injection destinations in Slice 1
production; their local client process is not proof of on-device model execution. Local destination
classes are runner-only fixtures selected after a verified loopback consumer observation. No caller
field or production setup option may select them.

Fixture metadata pins complete endpoints: literal-loopback HTTPS for the restricted local case and a fixed
remote HTTPS hostname for remote cases. The runner maps that hostname to its isolated loopback stub
and installs a per-run hostname/IP-matching public test CA into its private system trust before the
candidate starts. System chain/hostname
verification remains enabled; the public CA fingerprint is evidence-bound and no private key is
committed. Bind/CA/hostname mismatch fails rather than changing endpoint or provider fingerprint.

The corrected fixture statically defines complete manifests for:

- base remote OpenAI-chat endpoint
  `https://summary.stub.invalid/v1/chat/completions`, credential environment
  `FREE_MEM_SUMMARY_API_KEY`, derived `external_metered`;
- local derivation endpoint `https://127.0.0.1:1234/v1/chat/completions`, credential `none`, base/local
  ResourceProfile version 1 with derivation limit 16, as a complete successor bound to the base;
- one repaired remote successor at
  `https://summary-repaired.stub.invalid/v1/chat/completions`, bound to the base fingerprint and
  retaining ResourceProfile version 1/max16.

Configuration-activation, redirect-recovery, and downgrade-recovery signals target the repaired
successor's computed manifest/provider fingerprints. Runner cost evidence is 0 because the stub is
runner-owned; it does not change remote cost class.

Runner network evidence binds the public CA and exactly six raw credential/payload-free TLS
preflight receipts (base/local/repaired × setup activation/daemon start), including unique receipt ID,
hostname/SNI, port, frozen timeout, monotonic interval, verified result, trust-anchor fingerprint
equal to the public CA, and one peer-certificate fingerprint per endpoint that is identical across
its setup/start receipts and distinct from the CA, plus zero HTTP request, credential, and payload
byte counts. For each endpoint, setup preflight completion is strictly before daemon-start preflight
beginning.

## ProviderProposalV1

The accepted input is a closed object with exactly:

- `version: 1`, `role: summary`, `state: enabled`;
- `modelId` of 1-256 UTF-8 bytes and `modelRevision` of 1-128 UTF-8 bytes, with no ASCII control/NUL;
- `wireProtocol: anthropic_messages_v1 | openai_chat_completions_v1`;
- a complete canonical ASCII `endpointUrl` of at most 2,048 bytes including the final request path;
- `credentialRef: {kind:none} | {kind:environment,name:<valid environment name>}`.

Unknown fields, provider names/kinds, inline secrets, arbitrary headers, cookies, filesystem secret
paths, runtime-appended URL paths, and self-declared location/egress/cost/TLS/redirect/fingerprint
fields are rejected.

## Provider compilation

The compiler parses the URL once and requires `new URL(endpointUrl).href === endpointUrl`. Username,
password, query, fragment, empty/root-only path, and unsupported scheme are rejected.

- Literal `127.0.0.1` or URL hostname `[::1]` (the serialized form of `::1`): local; HTTP or HTTPS;
  `on_device`; `local_zero`;
  `not_applicable` for HTTP and `system` for HTTPS.
- Local HTTP requires credential `none` and is eligible-only. It never authorizes credential,
  private, or local-only bytes. Private/local-only processing requires local HTTPS whose exact peer
  passes chain and hostname/IP verification. Possession of the matching system-trusted certificate
  private key is the peer proof; PID/UID/port ownership is not authority, and a hostile port squatter
  without that key must fail preflight content-free.
- Any other host: remote; HTTPS only; `explicit_remote`; `external_metered`; `system` TLS.
- `localhost`, localhost subdomains, trailing-dot hostnames, wildcard/unspecified addresses
  (including IPv4-mapped unspecified), alternate loopback spellings, and DNS-to-loopback guessing
  are rejected rather than classified as local.
- Redirect policy is always `reject`; request code uses manual redirect handling and never follows or
  resends to a 3xx `Location`.
- `NODE_TLS_REJECT_UNAUTHORIZED=0`, an added CA path/environment value, or an equivalent trust
  override rejects production HTTPS activation/provider start. Production uses only platform system
  trust. The isolated runner provisions its test CA into its private system trust outside candidate,
  proposal, and manifest control; normal chain and hostname validation stay enabled.
- Before pointer/editor mutation and again at daemon start, every HTTPS choice performs a native
  credential-free, payload-free TLS handshake to the exact host/port/SNI with normal chain and
  hostname verification and a frozen 5,000 ms timeout. Setup failure leaves/restores the prior
  activation. Daemon-start failure does not abort writer/RPC/capture/spool-import/lexical startup; it
  disables provider/AI processing, retains pending work, and reports `provider_unavailable` or
  `provider_tls_rejected` until a validated healthy transition. No HTTP request, auth header, or body
  is sent. Local HTTP has no TLS handshake and remains credential-none/eligible-only.

The ProviderChoiceV1 contains the proposal plus those derived fields and
`providerFingerprint=sha256(domain || JCS(choice-without-fingerprint))`, where the domain is
`free-mem:provider-choice:v1\0`. Only the named credential environment variable may be read; its
value is never stored or fingerprinted.

## Frozen protocol profiles

ResourceProfileV1 fixes request timeout 60,000 ms, input 12,000 characters, output 4,000 tokens,
response 1,048,576 bytes, and temperature 0.2. Oversized responses are rejected before JSON parse.

Input characters are JavaScript UTF-16 code units. The user prompt reserves 25% (3,000 units): clip
system first to 9,000 units and call `toWellFormed()`, then give user
`max(3,000, 12,000 - clippedSystem.length)` units, slice from the start, and call
`toWellFormed()`. No tail merge, token-based alternative, or provider-specific reallocation exists.

- `anthropic_messages_v1` sends `content-type: application/json`, fixed
  `anthropic-version: 2023-06-01`, and `x-api-key` only for an environment credential. Its JSON is
  `{model,max_tokens,temperature,system,messages:[{role:"user",content}]}` and response text is the
  ordered concatenation of `content[]` text blocks.
- `openai_chat_completions_v1` sends `content-type: application/json` and `authorization: Bearer`
  only for an environment credential. Its JSON is
  `{model,max_tokens,temperature,messages:[{role:"system",content},{role:"user",content}]}` and
  response text is `choices[0].message.content`.

Credential `none` emits no authentication header. Redirects, streaming, Responses API, tier
routing, arbitrary headers, tool calls, and provider fallback are unsupported. Every request uses
the frozen timeout.

## Manifest closed shape

The effective manifest contains only:

- manifest version/id/base fingerprint and computed `configurationFingerprint`;
- the closed Claude Code/Codex destination map;
- exact ResourceProfileV1;
- one compiled ProviderChoiceV1;
- the explicitly disabled embedding lane with `semantic_disabled`;
- at most 64 unique legacy key dispositions (`translated | ignored | overridden`) whose names match
  `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`, with no values.

A detected legacy `conflict` rejects compilation and is not representable in an active manifest.
Secrets, content, prompts, arbitrary headers, absolute project paths, and unknown fields are
prohibited.

`translated` may copy only validated non-secret model/complete-endpoint/credential-environment-name
metadata. A legacy credential value, inline API key, auth file, arbitrary header, insecure TLS flag,
or implicit subscription source is never copied; it is `ignored` or `overridden` by explicit safe
input. Two incompatible effective candidates are `conflict` and prevent activation.

Manifest fingerprint input is the complete manifest after provider fingerprinting but without
`configurationFingerprint`, encoded as JCS and prefixed by
`free-mem:effective-capability-manifest:v1\0`. The stored value is
`sha256:<64 lowercase hexadecimal characters>` and must recompute exactly.
Job `admission_manifest_fingerprint` and `attempt_manifest_fingerprint` store this exact
`configurationFingerprint` value from the frozen manifest; they use no second domain or
recomputation path.

ResourceProfileV1 fixes all accepted fixture limits plus:

- worker warm lifetime 30,000 ms, periodic sweep 30,000 ms, idle flush 120,000 ms,
  event debounce 1,000 ms;
- stuck claim 300,000 ms, newly admitted v21 source events/job 100;
- raw-event retention disabled and 0 ms;
- observer request timeout 60,000 ms, max input 12,000 characters, max output 4,000 tokens,
  max response 1,048,576 bytes, temperature 0.2, and TLS preflight timeout 5,000 ms.

The only resource successor is the accepted test-only output-limit recovery fault manifest:
`profileId=slice1-short-run`; compared with version 1, only `version=2` and
`maxMemoryItemsPerDerivation=17` differ, with the base remote provider/destination/embedding/legacy
fields unchanged and the active version-1 manifest as base fingerprint. Production
setup exposes no resource selector and continues to compile version 1/max16. The runner may
materialize the complete v2 test successor. All other profile IDs/field overrides are rejected.

The manifest delivery removes mutable provider/scheduler reads and compiles their fixed values, but
keeps provider calls, AI maintenance, and RawEventSweeper execution disabled as
`pending_privacy_boundary`. v21 job and pack fields are reported as `pending_schema_v21` or
`pending_pack_boundary` until their independently mergeable deliveries; no early readiness claim is
allowed.

## Activation transaction

Setup is the only compiler and activation writer. It must:

1. reject unsupported platform/storage and compile the proposal/legacy translation without mutation;
2. show protocol, complete safe endpoint, credential source/name, model, derived location/egress/
   cost/TLS/redirect, provider fingerprint, and manifest fingerprint without a secret value;
3. obtain explicit confirmation without holding the lifecycle lock;
4. acquire the lifecycle lock, recheck writer/socket/health plus current editor/pointer prestates,
   and complete the payload/credential-free TLS preflight while held; abort before mutation on drift;
5. snapshot every targeted Claude/Codex editor file, manifest pointer, and mode-0600 activation-
   receipt prestate;
6. using fixed order `lifecycle -> setup/spool -> daemon writer`, hold the existing owner lock and
   persist/fsync one owner-only narrow setup transaction journal containing every target path/hash,
   including the activation receipt, and prestate needed for recovery but excluded from logs/
   manifest/evidence;
7. publish editor mutations, write the owner-only immutable generation, write the mode-0600
   activation receipt, and publish `current` last;
8. before rollback, classify every target against recorded pre/post state. If any target is unknown,
   mutate no target and retain the journal; otherwise restore/remove journal-owned poststate targets
   in reverse. Remove the journal only after commit or complete verified rollback.

Version-2 activation receipt IDs are UUID-shaped and canonicalized to lowercase at validation before
they become durable producer identities, so case aliases cannot bypass replay deduplication.

At next setup/daemon start, a leftover journal is finalized only if the intended pointer, activation-
receipt fingerprint, current generation, and every target hash match. If every target matches either
its recorded prestate or the journal-owned intended poststate, recovery may discard the receipt and
restore only journal-owned poststate targets to prestate in reverse. If any target matches neither
recorded state, recovery preserves every target unchanged, retains the journal, reports a bounded
recovery conflict, and blocks provider startup. Unknown external edits are never overwritten.

Daemon start takes the same lifecycle lock before journal/manifest resolution and writer-lock
acquisition and releases it only after startup state is published. No path takes lifecycle after a
writer/spool lock; setup preflight and activation therefore have no daemon-start race.

The activation journal records the prior current pointer. Rollback atomically restores and verifies
that fingerprint after coordinated lifecycle automation exists. Slice 1 defines no second rollback
pointer, and referenced generations are never deleted.

The first vertical PR includes compiler/storage, setup proposal/disclosure/confirmation/activation
and editor rollback, daemon snapshot/doctor, manifest-only ObserverClient, and manifest projections
for maintenance/viewer. Daemon consumption must not land before setup can produce the same manifest.
Full setup start/attach UI remains a later lifecycle PR.

## Runtime truth

- No `current`: daemon runs in explicit capture-only restricted mode with no provider and no sweeper.
- Unresolved/unrecoverable setup journal, malformed pointer, missing generation, fingerprint mismatch, unknown field, or invalid referenced
  manifest: daemon startup fails before provider construction.
- Valid current: daemon validates and freezes exactly one generation. The manifest-only
  ObserverClient factory/transport may be tested, but daemon does not construct an executable
  provider, start RawEventSweeper, or enable AI maintenance until v21 jobs and the complete
  DestinationBoundary are merged; doctor reports `pending_privacy_boundary` until then.

After privacy activation, a failed daemon-start TLS preflight is a provider-only degraded state, not
a daemon-start failure. Capture, spool import, RPC, writer, and lexical retrieval remain available;
provider work stays retained until the persisted healthy transition grants one retry.

Observer, sweeper, job attempts, pack destination resolver, doctor, status, maintenance, and viewer
receive that object or its fingerprints. They do not reread legacy provider/resource config or env.
Provider credential lookup is limited to the one environment name in CredentialRefV1.

`loadObserverConfig` and replay extraction are not public/runtime configuration surfaces. Legacy
observer parsing is setup-translator-only and yields dispositions; internal extraction replay is
test-only and uses ProviderChoiceV1.

Doctor and viewer configuration routes return the same content-free safe projection. Mutating legacy
env/config after daemon start changes no effective policy. Legacy reads may exist only inside the
explicit setup translator and must emit one non-secret disposition per recognized key.
