# Privacy and owner changes in the viewer

Settings W3 exposes the existing privacy and owner-claim operations in Japanese and
English. The settings page has one version-checked configuration save. Repository
exclusions, claim corrections, mute and global preferences have separate explicit
actions, so they do not submit unrelated configuration drafts.

## Configuration

The session-start terminal note is `[inject] session_start_note`. Its saved choice
applies at the next session start. The note contains no stored text.

`[backup] dir` follows the existing backup reader: a relative path starts at the
memory home and an absolute path stays absolute. Clearing the override selects
`<home>/backups`. A save creates no backup directory and moves no segments or
forget logs. Existing backups remain in their old location.

Additional redaction rules retain `id`, `regex`, `keywords`, `entropy` and
`secret_group`. The capture reader and `Rules::new` validate the complete candidate;
the bundled rules remain enabled. Invalid patterns, duplicate names, invalid
capture groups or invalid exception hashes leave the previous file unchanged.
Errors contain vetted codes rather than patterns or values. An older request
that omits these fields preserves their saved values, comments and defaults.
Changing rule names retains the saved tables and their comments; reordered rules
keep their submitted order. If names change together with additions or removals,
unmatched tables are retained in their saved order, so comments can move between
rules. Save name changes separately when their comment association matters.
The saved rule values always follow the submitted settings.
Clearing an optional rule field retains its comments
without retaining the old setting value. An unchanged repeated save preserves the resulting file bytes.

The exact-value exception control hashes UTF-8 in the browser, clears the live
input and sends only SHA-256 hashes with the configuration save. It preserves
whitespace and case. The value is the secret selected by `secret_group` (or the
first nonempty capture group), not necessarily the entire regular-expression
match. For a flattened tool field, use the exact escaped value being scanned.
Exceptions affect future masking; they do not restore already masked text or
remove existing range tombstones.

## Repository sends and rescan status

Excluding a repository stops future curation and embedding sends for sessions
touching it, including mixed sessions. Recording and search continue. Undo makes
future sends eligible again. Existing records, memories and backups remain.

Repository selectors are opaque hashes of the saved labels, resolved freshly on
the server. Display labels are redacted separately. Exclusions without history
remain listed for undo. The viewer's working directory is not a mutation target.

The rescan state reports the current append device. The worker rescans when it
next runs; saving rules does not start it. Rules-version equality alone does not
mean completion: the stored rescan version and checkpoint must also reach the
current raw sequence in the same snapshot. Progress is a sequence checkpoint,
not a count of surviving documents. A later rule change or rewind invalidates
completion. Missing derived state with existing raw data is pending; an absent
raw store is empty; an unreadable store or a restore in progress is unavailable.
Reads initialize no store or schema. This status does not claim coverage of other
devices, backup purging or physical erasure.

Parent-directory aliases are resolved before SQLite opens while the database
leaf still refuses symlinks. Original and opened paths retain the same file
identity under the raw swap hold. An explicit exclusion write may finish a
stopped restore's rename from `raw.db.restored` to `raw.db`; the reader accepts
that same file rather than accepting a replacement store or new identity. The
read connection closes before the recovery rename so Windows can move the file;
the original swap hold remains until the writer owns its hold and the pinned
identity is checked again.

## Owner claims

Claim details accept a correction and status change, mute or unmute. A muted
claim remains searchable and is excluded from memory handed to agents. Unmute
restores the existing eligibility rules. The global-preference form explicitly
confirms that the new preference applies to all repositories.

All actions reuse the CLI's backend and perform no inference. Full canonical
UIDs and repository selectors are protocol metadata; text passes the outbound
gate. The backend distinguishes a refusal before append, a recorded operation
awaiting application and an applied operation. Application requires actual
consumer output, including the retained correction or preference derivation.
Checkpoint passage by itself is insufficient.

A correction or mute of an unknown UID in an empty or absent home creates no
store and returns not found. Incomplete, unreadable or corrupt claim stores are
unavailable; these cases are not treated as an empty home.

A preference records an owner instruction before its claim operation. Failure
between these writes returns `directive_only` without a claim UID. The page also
treats a lost response as an unknown result: either may already be recorded.
It does not retry automatically and offers inspection before another preference.
Corrections, mute and preferences persist through rebuild.

During the current page session, preference drafts and sending/receipt state
survive tab changes and settings reloads. An applied preference clears its text;
pending, partial or unknown outcomes require explicit inspection or another
preference. A known pre-append refusal remains retryable. A recorded pending
claim correction or mute permits a further explicit owner change; an unknown
answer remains blocked. A send-exclusion acknowledgement followed by a failed
status read reports both the recorded write and the unavailable readback.

## Local API boundary

`GET /api/privacy` reads repository exclusions and rescan state. The typed writes
are `POST /api/privacy/exclude`, `/api/claims/correct`, `/api/claims/mute` and
`/api/preferences`. They share the viewer's Host, token, same-origin, JSON framing
and 16 KiB body guards before parsing or store work. Unknown fields and invalid
selectors are refused. Responses expose only canonical identifiers, typed state
and vetted codes; they do not expose backend error causes.

Capture exclusion, retention and the forget UI follow their milestone-5 slices.
