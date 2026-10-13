# The redaction switch (owner decision 43)

Plan of the slice that builds the switch decision 43 asked for: redaction on by default, and,
when the user turns it off, no redaction anywhere, as claude-mem's `CLAUDE_MEM_REDACT_ENABLED`
(off there by default). Written before the code, as the other slices' notes are.

## What the switch turns off

Every match of a redaction rule, bundled or the user's: in what capture stores, in what leaves
for a provider or an embedder, and in what agents and the page read (search, timeline, `get`, the
context at a session start). All of it reaches `redact::spans`: `scan`, `scan_capped`, `rescan`,
`gated` (the egress gate, its parts and lines), `ranges` and `field_ranges` (the rescan consumer
and forget's purge of op text) call it, and nothing else calls the matchers below it. So one
check there turns all of it off: with the switch off, `spans` finds nothing.

What stays, since none of it is redaction:

- `<private>` blocks and the like are removed at capture and at the egress gate, as before.
- Images and binary content are replaced by their marker at capture.
- A provider's error body is dropped; doctor and the settings page still never print a key, a
  rule's pattern or a kept value.
- A `[redaction]` table that does not load stops capture and sends nothing (the egress gate
  masks a text whole), as before: a broken table is a mistake, not the switch.
- The user's rules and kept values are still checked when the switch is off, so a rule saved
  while off cannot fail only when redaction is turned on again.
- What is already masked stays masked: `[REDACTED]` in a stored text is the text now, and a range
  tombstone keeps masking what it covers. Turning redaction off restores nothing.
- An output cut at the cap (2.4) still loses the half of a key block the cut leaves, as before: that
  is the cut, not a rule.

## Config and version

`[redaction] enabled`, a boolean; absent means on. `Rules` keeps it, and its version (the
ledger's `ruleset`) is `off` while it is off, whatever the rules and kept values: no hex hash is
`off`, so each reader that compares versions (the rescan consumer, the embedder's `masked:` marks
and its batches, forget's `ops_text_scanned`, the manifests' stamps, the settings page's rescan
state) sees a change of rules at each turn of the switch, and turning redaction on again rescans
what was stored while it was off, as adding a rule does now (2.2): the records get range
tombstones, masked on every read and in raw.db's rows at the next purge, and the op text is masked
at the next purge.

The rescan consumer keeps its checkpoint when the rules turn off: there is nothing for them to
find, so the pass from seq 1 that a change of rules starts would read every record for nothing.
It still writes `off` as the version scanned, so turning redaction on again starts that pass.
For the same reason a worker does not start forget's purge to scan the op text in the backups'
ops segments and the copies of raw.db set aside when the rules are off (3e-4): it records the scan
under `off`, and a purge that stopped still finishes.

Op text needs one more step. 3e-4 scans it only when the version of the rules changes, since an op
is gated with the rules of its time when it is written. An op written while redaction is off is
gated by no rule, and the switch can go off and on again before any worker reads the rules off:
the version is then the same as the last scan's, and the op would never be scanned. So an append
whose text was gated with the rules off owes the scan again, in its own transaction: it takes away
the record of the last scan, and the mark of a scan begun meanwhile, which then records nothing.
Each writer of op text says which rules it gated its text with (`Raw::gates_with`: an agent's work
state, a correction or a preference, the claude-mem import, v1's documents, a recuration from the
command or the page, the worker's curation and summaries), so this holds for a writer whose rules
were off when it read its text and the switch on when it wrote. The worker's look at the op text
begins the scan before it looks, so an append between its look and its record leaves the scan
unrecorded. The claude-mem import gates every field with the settings it started with, the rules
its appends owe the scan by.

A text gated under rules that are no longer the home's is not sent: the gate before each call to a
curator or a summarizer, retries and fallbacks included, compares the version the text was gated
under with the rules now, as the embedder's batches already do, and on a change the window is cut
again under the rules now (`ListChanged`), with no attempt counted. So redaction turned on while a
call waits sends nothing gated while it was off; a rule added meanwhile is held the same way.

`Rules::default()` stays the bundled rules, on. The one place that stores text with them rather
than with the home's settings is the claude-mem import's prompts (`hook::clip`): it keeps them, so
its check for a key block it cuts sees every header (a user's rule that masked a header would hide
it), and follows the switch: while it is off, a prompt is kept as typed, cut as before.

## The page, doctor, the CLI

- The settings page shows the switch at the top of its masking section, on by default. Turning
  it off shows, in both languages and before the save, what it means: from the save on, secrets
  are stored as typed on this device and in its backups, and sent to the AI providers and
  embedders set up, which cannot be called back; turning it on again masks what was stored
  meanwhile at the worker's next run and in the stored rows at the next cleanup, but not what
  was sent. The section's text no longer says the built-in rules always stay on.
- A request that does not name the switch (an older page) keeps its saved value, as for the other
  privacy fields.
- doctor's capture line says `redaction off` when it is, in place of the rule counts.
- No CLI command: the switch is the config key and the page, as the other `[redaction]` keys.

## Tests

- Off: `scan`, `outbound_with`, `lines_with` and `hidden` return the text as it is (blocks
  removed); `version()` is `off` whatever the rules; a bad rule still fails `Rules::new`.
- The rescan consumer: a record stored while off, then the switch on, gets its tombstone; turning
  it off does not start a pass from seq 1.
- The page: the switch saves and reads back; a request without it keeps the saved value; the
  warning shows while it is off.
- doctor's line.

## Docs

spec 1.5 and 2.2/2.5 (built-in redaction is no longer fixed; decision 43), the parity table's row
(decision 37 left the switch out; 43 builds it), `settings-privacy.md`, `settings-coverage.md`.
Security scope (rules/security.md): reviewed as such before merging.
