# Memory core contracts

## Source acceptance and generation

Keep capture's quiet/exit-zero contract. Acceptance means sanitized material was durably stored
in SQLite or recoverable spool. Failed acceptance preserves existing data, reports the bounded
diagnostic and lets coding continue.

Requests are bounded and identify the exact source portions represented. Unsent portions stay
pending. The model accounts for considered portions through observations or explicit no-memory
outcomes. Syntax alone does not prove useful recall. The worker records fixed-code outcomes and
applies their effects in the same fenced transaction.

Fallback may create labeled temporary guidance. Its sources remain eligible after the chosen
provider recovers. Repetition cannot duplicate effects, revive tombstones, weaken sensitivity
or reuse consent for a changed destination. Retry due dates bound work; retry happens during a
later normal worker invocation. The original increment-A implementation is bounded; the approved
2026-09-10 amendment adds resident waiting for due retries in T047 while retaining this one-shot path.

For the first checkpoint, retry is five minutes after the first settled failure/unaccounted
attempt, doubling to a maximum 24-hour interval. Each attempt records source membership/outcome.
Only fully represented sources with accepted provider accounting can complete; a noop requires
an explicit nonempty reason. Partial/omitted sources remain pending. Existing terminal raw from
before migration stays recoverable but requires an explicit reprocessing choice to avoid a
surprise historical paid backlog. Pending sources already queued before migration continue.

Processed full activity stays 30 days from processing. Pending accepted material is never
age-purged. Important supporting portions stay with knowledge; user deletion remains
authoritative. Previously purged source data is reported unavailable, never fabricated.

### Bounded processing pages

An attempt contains at most 50 summarizable sources and normally at most 2 MiB of stored text;
one accepted oversized source may occupy a page alone. Eligibility is computed across the due
session before paging, including ten distinct turns; metadata/blank rows cannot starve content.
The request gives sources the 12,000-character budget before nearby previews. Whole events keep
their original shape. An event that cannot fit alone travels as contiguous UTF-16 ranges of its
canonical `event-json-v1` serialization, without cutting a surrogate pair. The fragment names
the original event ID, whole-source SHA-256, start/end/total and exact text. Free summaries are
already events and are not repeated outside this accounting. Turns describe only sent sources.

Each receipt stores the exact range and hash sent. A fenced compare-and-set advances the stored
cursor only from the acknowledged start to end with the same source hash. Full completion starts
30-day retention; acknowledged partial progress and unsent sources remain pending for another
page. Failed/unaccounted/rejected portions wait with backoff. A crash before apply repeats the
same range. Sources not yet advanced by this worker precede its own advanced sources, so a large
event cannot monopolize the run. Changed sanitized content restarts its own cursor at zero.
Batch creation claims at most 50 sources globally per transaction. The existing through-event
watermark lets an eligible ten-turn cohort drain across pages. Older oversized pending batches
are detached by indexed SQL and repaged before any provider request reads them.

Classification reads one bounded page per worker pass. Every actual batch also rechecks all
source text, decoded tool input and paths, including cached eligible/private sources, against
current credentials and global/repository rules. Nearby memory bodies receive the same check.
Detection failure defers the source; a newly found secret clears raw content, normalized payload
and independent evidence and quarantines derived material. The final request is checked again,
and a policy/credential fingerprint is compared immediately before every send and language retry.
Captured roots and detector paths accompany raw/spooled sources and independent evidence. Root
identity is checked against the recorded repository before policy lookup and again before send;
an unavailable or reused origin is held. Relative paths resolve against that source root.
Nearby admission checks source/citation paths and its memory hash, text, sensitivity, quarantine
and deleted state after detection and immediately before send. Secret quarantine clears evidence
and citation payloads; detector failure cannot lower a concurrent privacy upgrade.

Session summaries wait until due pages have drained. They read bounded source IDs, first/last
prompts and aggregated path/title lists with omitted counts, never all source/memory bodies.
Computation holds a read snapshot; concurrent capture is permitted and causes a stale snapshot
to retry on write upgrade. The summary and session pointer still commit under one lease fence.

Accepted memory citations retain the exact sanitized sent portion, range, source hash and
capture time in existing `memory_sources`; explicit no-memory decisions retain only the receipt.
Evidence and memory effects commit with cursor progress. Raw purge requires independent evidence
for ordinary knowledge; legacy knowledge without such evidence still holds its raw source.
Temporary guidance does not become ordinary until all its source groups complete. Per-session
summary health is independent of content-deduplicated memory health. Secret reclassification
quarantines derived material and clears retained source payloads.

Recovered old observations cannot update/delete a newer or undated nearby record. Keep an old
update as historical evidence pointing to the newer record, and record an old delete as a
non-mutating historical decision. Source capture time determines this order; processing time
does not. Unmatched contradictions remain a separate recall-quality acceptance test.

`doctor` reports source processing independently from provider connectivity. `why <session-id>`
reports at most 100 retained source IDs with cursor/outcome/retry metadata, never raw or provider
bodies. Receipts retain turn membership and fixed historical-update/delete decisions, including
the protected target and capture-order reason. After raw expiry they report unavailable source
content with the retained range; missing legacy membership is explicitly unknown.
`observe --reprocess-source <source-id>` explicitly restarts one complete retained source,
including legacy sources, under the current selected provider and consent. Other historical
sources stay held. Incomplete captures stay waiting with no automatic retry time; their missing
material cannot be recreated by repeatedly processing the accepted prefix.
The same retry backoff applies to detector failures. The worker checks its run deadline before
each inherited or newly created batch and before starting a summary, then releases its lease.

## Work and visibility

Session/work binding and native resume/compaction lineage are distinct. Automatic continuation
requires an unambiguous match. An ambiguous pack contains short choices and applicable knowledge,
without mixing active checkpoints. Selection is persisted. Merge can adopt reusable knowledge
but cannot close outstanding work.

Search, injection, MCP and viewer apply one visibility rule before formatting. Personal shared
projections contain approved generic statements, not project source payloads. Broader scope
never relaxes detection or destination consent.

## Transfer and cost

Migration previews counts/mappings, reads sources without mutation, applies atomically and is
repeatable. Unknown mappings and historical progress stay unresolved/historical. Encrypted
sync uses the same validated merge with stable origin/parent revisions and visible conflicts.

Users select local/free/paid/supported agent CLI. Setup explains destination, credentials, cost
and data classes. Free/local failure never selects paid service. Paid mode has a configured
policy; local estimates and provider hard caps are distinguished.

## Compatibility and completion

Keep existing hook envelopes, quiet capture and privacy contracts. New schemas require matching
hook/worker bundles. Version-3 workers refuse schema 4, but version-3 hooks lacked an ahead-schema
guard; the current bundle adds that guard for both roles. A new export retains the old
reader. Add user-facing commands only with their implementation and parser/help tests.

Deterministic tests prove storage invariants. Full completion additionally requires real-model
Japanese/English quality, twelve ordered agent pairs, actual Linux/WSL/macOS, replica fault
recovery and seven-day use. Missing credentials/hardware remain unverified, never passing.
