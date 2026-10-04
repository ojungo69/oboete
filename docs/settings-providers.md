# Provider entries and explicit connection tests (W2)

The Japanese and English settings page edits individual provider entries separately from the
existing name-group controls. Opening the page, editing settings or registering a key sends no
model request. A connection test first shows its fixed fixture, destination, model, possible cost
and current budget readiness; a separate button sends one request.

## Individual configuration

The page supports OpenAI-compatible HTTP entries and the existing Claude and Codex CLI adapters.
It edits name, model, timeout, on/off, token limits and HTTP prices, and provides add, remove and
ordering actions. Only priced HTTP calls enforce a normal output cap, so that control appears
only when an HTTP price is positive. The page explains the existing unforced estimate for other
entries. Subscription entries offer no new daily-call cap; a legacy cap survives other edits.

Each operation identifies the source and native array index plus the configuration version.
Duplicate names remain separate entries. Name-group overrides still determine effective order,
model, timeout and on/off; the page shows saved and effective values apart. Editing a built-in
entry materializes the native list. Editing a derived Gemini entry materializes it; removing the
final Gemini entry also removes its placement, while another native Gemini keeps it.

The typed backend holds the shared configuration lock, preserves comments, unknown fields and
private extras, validates the whole candidate, stages it and compares its version before
publication. A stale tab or failed write leaves the previous usable configuration. HTTP extras,
headers, credential paths and process arguments are retained from existing entries rather than
accepted from an editor. HTTP endpoints require HTTPS or numeric loopback HTTP and reject URL
credentials, queries, fragments, malformed hosts and invalid ports.

## Write-only credentials

Linux registration creates a new opaque `*_KEY.md` under the process owner's local-data directory,
outside the corpus, with owner-only directory and file permissions. If that location is unsafe,
the existing owner-home fallback must satisfy the same ownership, ancestor and filesystem checks.
The request supplies an entry selector and key, never a filesystem path. A config failure removes
only the newly created unused file; it does not overwrite or delete an existing credential.

The browser clears the key field before sending. GET, response bodies, errors and the call ledger
contain no key value. Registration state and connection-test success are separate. macOS and
Windows retain the existing owner-only-storage refusal tracked by #281.

## One explicit request

Preview is read-only and creates no configuration, ledger, credential, process or discovery
request. Execute requires affirmative intent and the preview's saved selector/version. The viewer
applies the existing token, Host, Origin, framing, JSON and body guards. The actual sender rechecks
the fresh egress gate after CLI preflight; a changed configuration or redaction rule can refuse it.

The test sends only `Return exactly this JSON object: {"ok":true}` with its fixed schema. HTTP
uses a small output bound, no streaming, no redirects, retries or fallback. Fixed CLI calls reuse
the existing isolation, checked headless arguments, private home/scratch and bounded parsers; their
preview reports the existing conservative CLI output estimate rather than promising an enforced
HTTP bound. Results contain latency, vetted status and budget information, without provider output
or error bodies. A test does not resume an owner-held provider or change configuration.

## Budget and rollback

Normal Chain calls and tests reserve allowance in the same short database transaction before
dispatch. Pending reservations count against rolling calls/tokens and monthly curation spending.
Settlement updates that one row with reported usage and cost, reads current provider state and
preserves a concurrent owner hold. A proved unsent request releases its reservation; a crashed
reservation remains conservative. No transaction remains open during network or CLI work.

For sent calls with missing usage, settlement retains the admission-time input/output bounds.
Later calls with smaller bounds or changed prices/calibration do not shrink that retained token
estimate. Legacy rows without such bounds keep the previous conservative fallback; their unknown
historical bounds are not reconstructed. A probe uses the normal entry's current fallback for
those old rows, and its own smaller bound for the new request. Probes consume limits and spending but do not calibrate
the differently shaped normal curation prompt. Embedding keeps its separate pool policy.

Rollback reverts the W2 commit. Credential files and the provider ledger remain; rollback does not
delete keys, reset recorded costs or change the owner's installed runtime.
