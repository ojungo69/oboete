# Privacy and owner changes in the viewer

Settings W3 exposes the existing privacy and owner-claim operations in Japanese and
English. The settings page has one version-checked configuration save. Repository
exclusions, claim corrections, mute and global preferences have separate explicit
actions, so they do not submit unrelated configuration drafts.

## Configuration

The session-start terminal note is `[inject] session_start_note`. Its saved choice
applies at the next session start. The note contains no stored text. Above it,
`[inject] session_start_list` (on by default, decision 51) shows the memory list the
agent got, which is stored text, as claude-mem's terminal timeline is; off, the note
is shown alone.

`[backup] dir` follows the existing backup reader: a relative path starts at the
memory home and an absolute path stays absolute. Clearing the override selects
`<home>/backups`. A save creates no backup directory and moves no segments or
forget logs. Existing backups remain in their old location.

The masking switch, `[redaction] enabled`, is on unless the file says `false`
(owner decision 43). Off, no rule runs, the bundled ones included: secrets are
stored, sent and shown as typed from the next recording, send or display on.
The page shows that warning, in both languages, as soon as the box is cleared
and before the save. A request that does not name the switch keeps the saved
value. Turning it on again rescans this device's records at the worker's next
run, as a new rule does; what was sent while it was off stays sent.

Additional redaction rules retain `id`, `regex`, `keywords`, `entropy` and
`secret_group`. The capture reader and `Rules::new` validate the complete candidate,
also while the switch is off; the bundled rules cannot be removed one by one. Invalid patterns, duplicate names, invalid
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
the server. Newly captured raw repository metadata is listed before the worker
indexes it, together with derived/imported labels and exclusions without history.
Display labels are redacted separately. Exclusions without history remain listed
for undo. The viewer's working directory is not a mutation target.

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

## Never record

The page's "Never record" section is `oboete capture exclude`'s backend (spec 6.1,
docs/milestone-5-plan.md slices 4a and 4b). Each listed repository can be set to
never be recorded, or recorded again; a folder of this device is added by its
absolute path and listed with its own selector for taking it back. Nothing that
happens there is written from then on, and a repository never recorded is never
sent either: its send row says so and offers no undo until recording is allowed
again. What was recorded before stays; the receipt names `oboete forget --repo`
for a repository, with a folder in it rather than the label the page shows, which
a redaction rule may mask. An undo's receipt says the exclusion is removed and that
recording follows from then on unless another exclusion or a checkout's
`.oboete.toml` still covers the place; for a folder that another entry still covers
(a listed folder around it, its repository, or its checkout's `.oboete.toml`) it
says nothing there is recorded yet, naming no path, as the list shows the paths
masked. What it answers is read before the write, so a write is never answered as
failed after it committed.

A repository shows as never recorded when a hook would not record it now: its
label, or that label as the rules now read it, matches an exclusion as they read
it, so a rule added since an exclusion may cover another repository it now reads
alike. Its undo does not take back an exclusion stored for another repository,
which would record that one too: it is refused (`never_other`, or `never_ambiguous`
when several exclusions read as it now), as `oboete capture exclude --undo` refuses
it, and those exclusions keep rows of their own, each taken back from its row. An
exclusion whose repository's records carry only the label a rule made of it is
listed under that label, when no other listed repository reads as it, and that
row's undo takes it back. An undo's answer says when the repository stays
unrecorded because another exclusion still reads as it.

A repository is named by the same opaque selector as the send exclusion, resolved
freshly on the server, and stored as its records name it, the one name the page has
for it; a folder is stored as typed, as `oboete capture exclude --folder` stores one.
What is never recorded is stored as given, so it holds whatever the redaction rules
become (owner decision 43), and a hook compares it as given and as the rules now
read it. A folder must be an absolute path of at most 4,096 bytes;
it is stored as the CLI stores it (`.` and `..` taken by name), through the gate
as an event's `cwd` label, and its selector is a hash of that stored label. A relative path, an unknown selector, or a request that
names more than one target is refused before any write.

The first-run guide says to name such places before their next session and
offers a button to this section.

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

`GET /api/privacy` reads repository exclusions, what is never recorded and rescan
state. The typed writes are `POST /api/privacy/exclude`, `/api/privacy/capture`,
`/api/claims/correct`, `/api/claims/mute` and `/api/preferences`. They share the viewer's Host, token, same-origin, JSON framing
and 16 KiB body guards before parsing or store work. Unknown fields and invalid
selectors are refused. Responses expose only canonical identifiers, typed state
and vetted codes; they do not expose backend error causes.

Retention and the forget UI follow their milestone-5 slices.
