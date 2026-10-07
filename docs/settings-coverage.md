# The settings page: coverage and the slices that close it (#94)

[spec-webui.md](spec-webui.md) asks for a coverage matrix as the closure evidence of #94: every
user setting and operation, where the page offers it, what it calls, when it takes effect and its
test. This is that matrix, kept with the work. It is not a second source of configuration: the
settings are config.toml's and the operations the CLI's, through the same typed backend
(`src/settings.rs`, one validation and one atomic, stale-checked write).

Owner, 2026-10-03: the settings page is complete at the switch (「設定画面は切り替え時に全部」), and
sync and the cloud come after it. So every row below is built before the switch, except those of
"Sync and devices", which follow milestone 6, and the updater, which milestone 7 builds (7.3); until
then the page shows the installed version and says the update is not built, as spec-webui.md asks
of a feature not built yet. The switch is on the owner's WSL machine, so "at the switch" is read as
"on Linux": where a row's other release targets wait for #281 (macOS and Windows), they are a row of
their own that #94 waits for after the switch, not a reason to hold the switch.

## The matrix

"Now" records the implemented coverage below, updated by each completed slice. "Takes effect"
is when a saved value is used: the worker reads its
settings again for each window it curates, a hook reads its own at each event, and a key file is
read at each call. "OS" is where the row works; "all" is WSL/Linux, macOS and Windows, with the
real-machine checks spec-webui.md asks for at the end. "Test" names the test that proves the row,
in `src/` unless a file is named; a slice fills in its rows' tests, and #94 closes when no row is
left with "—" there.

### Curation and spending

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| A provider's order, on/off, daily budget, timeout, model | 1.4, 1.5 | page | `[chain]` | the next window | all | `settings.rs` `a_save_writes_only_what_changes`, `a_refused_save_leaves_the_file`, `a_model_the_entry_cannot_price_is_refused`; `view.rs` `a_settings_save_passes_every_guard_first` | built |
| A provider's key, write-only | 6.6 | page; managed registration per entry | `save_key`, `save_provider_key` | the next call | Linux | `settings.rs` `a_key_saved_from_the_page_reaches_its_file_and_no_answer`, `no_key_is_shown_or_taken`; `settings/providers.rs` `managed_key_registration_binds_a_physical_entry_without_revealing_or_replacing_old_keys`, `a_failed_config_save_discards_only_the_new_managed_key`; `keyfile.rs` `managed_registration_rejects_bound_corpus_descendants_before_mkdir`, `managed_registration_requires_complete_bounded_mount_observations`; `view.rs` `a_key_save_answers_without_the_key` | built; W2 |
| A provider's key on macOS and Windows, in owner-only storage there | 6.6, spec-webui | refused (`a_key_save_waits_for_owner_only_files_off_linux`) | `save_key` | the next call | macOS, Windows | — | #281, after the switch, before #94 closes |
| Add, edit, remove, order and disable a provider entry of a supported type | 1.4 | page, apart from name-group overlays | `[[providers]]`, `save_provider` | the next window | all | `settings.rs` `individual_provider_edits_keep_duplicates_unknowns_and_stale_config`; `settings/providers.rs` `provider_edits_keep_native_noops_inline_fields_and_legacy_subscription_caps`, `editing_other_native_fields_keeps_legacy_http_caps_but_rejects_new_invalid_caps`, `moving_native_tables_keeps_their_nested_fields_and_unrelated_comments`, `builtin_edits_materialize_the_native_base_without_losing_effective_overlays` | W2 |
| Test a provider's connection, separately from a save | spec-webui | page, preview then explicit single-entry HTTP test; CLI probe refuses an unprovable output cap | `preview_provider_test`, `test_provider` | at once; test result and spending only, no config save | all | `settings/providers.rs` `a_probe_preview_is_bound_to_saved_selection_and_makes_no_files_or_requests`, `a_cli_probe_preview_is_truthfully_unavailable_without_ledger_or_process_work`, `a_probe_does_not_shrink_the_normal_fallback_for_legacy_unmetered_calls`; `provider.rs` `explicit_probe_sends_only_the_fixed_fixture_and_returns_no_provider_payload`, `explicit_probe_rejects_redirects_and_http_refusals_without_retry`; `budget.rs` `concurrent_reservations_share_daily_calls_tokens_and_the_curation_month`, `a_small_probe_cannot_shrink_a_previous_unmetered_curation_call`, `reservation_pressure_backs_off_after_restart_without_refunding_it`; `view.rs` `a_settings_save_passes_every_guard_first` | W2 |
| Curation on or off | 3.1, 7.4 | page | `[summary] curate` | the next window | all | `settings.rs` `the_summary_shows_saved_values_and_the_parser_ranges`, `a_summary_save_is_lossless_and_stale_checked` | W1 |
| Summary language, window size, idle wait | 3.1 | page | `[summary]` | the next window | all | `settings.rs` `a_summary_save_is_lossless_and_stale_checked`, `the_paid_cap_takes_finite_values_of_0_or_more_only` | W1 |
| The monthly cap of paid calls, with the month's spend beside it | 1.4, parity 13 | page | `paid_usd_per_month`, providers.db | the next call | all | `settings.rs` `the_paid_cap_takes_finite_values_of_0_or_more_only`, `the_spend_and_the_owner_stops_are_read_without_a_write`, `a_top_level_setting_changed_keeps_the_comments_above_it` | W1 |
| Gemini's place in the chain | 1.4 | page | `gemini` | the next window | all | `settings.rs` `geminis_place_follows_the_config_chain`, `taking_gemini_out_keeps_its_comments` | W1 |
| Resume a provider stopped for the owner | 1.4 | page | `oboete resume` | the worker's next pass | all | `view.rs` `resume_passes_the_save_guards_and_only_clears_the_stop`; `settings.rs` `the_spend_and_the_owner_stops_are_read_without_a_write` | W1 |

### Delivery, capture and privacy

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| Session start and per-prompt injection, on/off and size; correction notes | 4.4, 4.6, 4.8 | page | `[inject]` | the next session start or prompt | all | `settings.rs` `inject_settings_check_ranges_and_save_alone` | built |
| The terminal line at session start, on/off | parity 9 | page | `[inject] session_start_note` | the next session start | all | `settings.rs` `terminal_note_is_read_only_and_an_omitted_old_field_keeps_its_choice` | W3 |
| Prompt storage, tool output detail | 2.3, 2.4 | page | `[capture]` | the next record | all | `settings.rs` `a_save_writes_only_what_changes`, `a_refused_save_leaves_the_file` | built |
| Exclude a repository from curation and embedding, and take it back | 5.5 | page, saved-label selectors and exclusions without history | `oboete exclude`, `settings/privacy.rs` | the next eligible send | all | `settings/privacy.rs` `stored_repo_selectors_survive_rules_and_exclusions_without_history_can_be_undone` | W3 |
| Stop recording a repository or folder | 1.5, 6.1, parity 14 | — | capture exclusion (M5 slice 4) | the next record | all | — | W3 after M5 slice 4 |
| Additional redaction rules and allowed exceptions | 2.2, 6.4 | page, with current-device rescan checkpoint and state | `[redaction]`, `consumer::rescan` | the next record/send/display; the worker rescans at its next run, adding range tombstones (2.2) | all | `settings.rs` `redaction_saves_keep_all_rule_fields_and_reject_invalid_candidates_without_side_effects`; `settings/privacy.rs` `rescan_status_tracks_real_batches_rules_and_a_rewind_without_unmasking`, `a_fresh_privacy_get_is_empty_and_creates_nothing` | W3 |
| Raw retention | 6.2 | — | retention (M5 slice 3) | the next retention pass | all | — | W3 after M5 slice 3 |
| Backup location | 2.6 | page, preserving old files and logs | `[backup] dir` | the next backup | all | `settings.rs` `backup_choices_preserve_old_requests_and_move_no_existing_data` | W3 |
| Correct, mute, unmute a claim | 3.4, 6.1 | claim details; recorded/pending/applied receipt | `oboete correct`, `mute`, shared recorded backend | application is confirmed, or explicitly pending | all | `settings/claims.rs` `w3_correct_applies_the_gated_owner_body_and_status`, `w3_mute_and_unmute_preserve_search_and_change_injection_eligibility` | W3 |
| A preference for every repository | 4.4 | page with explicit global confirmation; partial recording distinguished | `oboete pref`, shared recorded backend | apply after consumer confirmation; delivery at the next eligible session start | all | `settings/claims.rs` `w3_explicit_global_preference_is_applied_without_inference` | W3 |
| Forget, with its preview | 6.2 | — | M5 | at once, after the preview's confirmation | all | — | M5 slice 5 |

### Search and embeddings

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| Embeddings: none, local or Workers AI, with caps | 4.10, 7.1 | — | `[embedding]` | the next window; a new space builds a new generation | all | — | W4 |
| Workers AI's account id and token, the token write-only with its registration state | 4.10, 6.6, spec-webui | — | `[embedding] account_id`, `key_file`; `save_key` for the embedder's entry | the next embedding call | as the key rows | — | W4 |
| The local model: source and size before the download, progress, readiness | spec-webui | — | local embedder (parity 5) | once downloaded and checked | all | — | W4 after the local embedder |
| Index generations: building, active, rebuild after a change | 4.10 | — | `vec_generation` | when the generation is complete | all | — | W4 |

### First run, agents, import and maintenance

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| First run: an unconfigured home, recommended presets | spec-webui | resident preset on Linux/WSL; full wizard — | settings | explicit save | all; resident Linux/WSL | `settings.rs` `resident_settings_are_read_only_until_the_visible_choice_is_saved`; native browser first-save/off/reload | resident slice 5; remaining W6 |
| Detect agents, wire and unwire their hooks | 7.2 | Agent registrations: passive seven-agent file inventory; wire/unwire — | shared `setup::readiness`; `oboete setup` for later confirmed writes | inventory on page load or explicit Refresh; wiring later | all; private native fixtures on Linux | `w6_` public guarded GET and native-file registration matrix; private HTTP/FIFO/source-purity checks; JA/EN DOM/latest-read/unchanged-draft checks | W6 first inventory slice; wiring remains |
| Readiness checks, as doctor reports them | 7.2, R13 | passive agent inventory page; resident status CLI; full readonly Doctor page — | `setup::readiness`; `oboete doctor` for later query extraction | inventory at once, read only; full checks remain | all; resident Linux/WSL | `w6_` invalid-config/no-store/secret-response checks; `tests/doctor_curators.rs`; `tests/resident_defaults.rs` `doctor_reads_resident_locks_and_outcomes_without_starting_or_renumbering_them` | resident slice 5; W6 inventory; full Doctor remains |
| Import claude-mem's history, with its projects mapped to repositories | 7.4, parity N1 | — | `oboete import` | when confirmed | all | — | W5 after N1 |
| Migrate v1's store; import transcripts | 7.4 | History import: candidates/settings preview, explicit consent, committed progress and partial receipt | shared `migrate`/`transcript`, `settings/maintenance.rs` | confirmed import; later processing uses saved settings | all | maintenance preview/start/status, native CLI compatibility and JA/EN import journeys | W5A |
| Recurate, with the list and estimate before the send | 1.7 | Typed local no-send preparation, scope/work/cost consent, own provider/window progress and partial receipt | shared native `curate::prepare_report`, `recurate_report`, ordinary Chain | explicit preparation; send only after confirmation and fresh admission | all; pinned resident caller on Linux | `w5c_` native/guarded regressions; synthetic HTTP send/replay, concurrent probe, settlement and later-index failure; actual same-PID Viewer exec and JA/EN browser consent/fractional-USD journeys | W5C |
| Rebuild, restore | 1.7, 2.6 | History and recovery: current-data/backup preview, explicit consent, native progress and recovery receipt | shared complete `worker::rebuild_report`, `restore_report` | confirmed native operation; later background work follows saved settings | all; R9 borrowed viewer on Linux | `settings/maintenance.rs` fixed-operation and receipt replay; `w5b_` regressions for home proof/replacement, disappearing WAL, durable progress, segment/file counts and same-metadata/ABA log consent; native CLI and foreground/resident JA/EN journeys | W5B |
| Finish v1 migration and remove old files | 7.4, 7.5 | Read-only exact-target preview, final-import/deletion consent, causal counts and inspection of partial/uncertain results | shared CLI/HTTP `migrate::finish_report` | final import before confirmed deletion | all; pinned resident caller on Linux | `w5c_` pure/WAL/link/FIFO/backup/identity/self-stale/partial tests; actual HTTP complete/replay/current-rule label gates and permission-failure receipt; JA/EN browser complete/partial and saved-rule display journeys | W5C |
| Update check and update | 7.3 | — | `oboete update` (milestone 7) | when confirmed | all | — | after the switch, with milestone 7; W5 shows it as not built |
| The resident worker and viewer: on or off | 1.8, A111 | page; setup fills absent defaults | `[worker] resident` | on: next hook or `oboete view`; off: worker idle, viewer minute tick without requests | Linux/WSL | `settings.rs` `resident_settings_are_read_only_until_the_visible_choice_is_saved`, `resident_saves_reject_wrong_types_stale_versions_and_invalid_runtime_settings`, `a_settings_save_waits_for_the_other_config_writer_then_refuses_its_stale_body`; `migrate.rs` `migrated_settings_wait_for_the_config_writer_and_keep_its_choice`; `tests/resident_defaults.rs`; native browser first-save/off/reload | resident slice 5 |
| Resident page port and a new page token | 1.8, 6.6 | CLI; page — | `[view] port`, `oboete view --new-token` | port: viewer's next start; token: at once | Linux/WSL; other OS token storage waits for #281 | resident.md slice 3 CLI tests; page — | remaining W6 |

### Sync and devices (after the switch, milestone 6)

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| Deploy or connect the hub; add and remove devices; raw sync and its exclusions | 5 | — | milestone 6 | when confirmed | all | — | M6 |

## The slices

Each slice is one PR, built test first, with its page parts in Japanese and English and checked in a
browser. Each keeps spec-webui.md's rules: saved, effective and pending values shown apart; a save
that fails leaves the previous configuration; nothing that costs money, downloads or reaches a
cloud starts from a page view or a save.

- **W1, curation and spending.** The `[summary]` fields, the paid cap with the month's spend (the sum
  of the month's call costs providers.db records, as doctor counts them), Gemini's place, and
  `resume` per stopped provider.
- **W2, providers.** Add, edit and remove entries of the supported types, and an explicit connection
  test that names its destination, sends a small synthetic request and says what it may cost first.
  Security scope: endpoint and key handling follow the provider safety rules (`provider.rs`).
  The saved native entry is separate from its effective name-group overlays; only priced HTTP
  offers a normal output cap. [settings-providers.md](settings-providers.md) records the supported
  fields, credential storage, probe and shared budget contract.
- **W3, privacy and claims.** The terminal line at session start, exclusions, redaction rules and
  exceptions (validated as config.toml's are, with the rescan an added rule starts and its state),
  the backup location, correct, mute and unmute, the global preference; then capture exclusion and
  retention as M5 builds them.
  [settings-privacy.md](settings-privacy.md) records the readonly rescan, exact-value hashes,
  repository selectors and truthful owner-operation receipts. The capture, retention and forget
  rows retain their milestone-5 dependencies.
- **W4, embeddings.** The embedder's choice and caps, Workers AI's account id and write-only token, the
  local model's download with its consent and progress, and the generations' state; after the local
  embedder (parity 5).
- **W5, import and maintenance.** The imports with their previews, recurate with its estimate,
  rebuild and restore with their confirmations and progress, and v1 finalization with its exact
  deletion scope. The updater stays visibly not built until milestone 7 builds it.

  W5C preparation of recuration is an explicit no-send POST: native recovery, redaction/rescan,
  backup and indexing may commit locally. No provider ledger is created to price an absent one.
  Confirmation binds the own-device scope, exact requests, configured prices/calibration, rules
  and full source content/identity. Original caller proof is checked before worker admission and
  again under it; no store handles or locks survive the browser's confirmation gap. A stale
  confirmation performs no admitted drain. A hook during that refusal can need the next hook,
  view command or explicit preparation to index it, as the existing R12 stale boundary does.
  A config-lock failure after acquiring the worker hold still attempts the reported completion
  drain under the original home and valid consent; stale consent continues to refuse that drain.

  The paid-chain estimate is an estimate, not a new operation spending cap. Execution keeps the
  ordinary shared live budget, privacy and dispatch gates. Receipts count this operation's own
  reservations, possible sends and settlements; a concurrent connection test is excluded.
  Accounted USD includes outstanding bounds, replaces them on settlement and removes them only
  after proven-unsent cancellation. It is not an invoice. A later index failure retains a
  committed window and its provider accounting. Unknown settlement never implies a refund.

  Finalization preview copies v1 DB/WAL privately and binds the native whitelist's full recursive
  membership, entry identities, contents and link text without following link targets. It refuses
  overlap with configured native backups. Metadata bounds are 128 top targets, 16 KiB of original
  labels, 4 KiB per relative/link path, depth 32 and 8 MiB of buffered child hashes. File hashing
  streams 64 KiB; there is no total file-byte/node cap or silent truncation. Windows uses native
  no-follow entry identity. An unsupported identity or exceeded metadata bound refuses preview.

  Confirmed finish shares the CLI's final pass and keeps its Raw swap hold through CLI yes/no/EOF.
  It copies no settings and performs no inference or knowledge-index drain. Owned Raw/checkpoint
  changes do not invalidate consent: after import, only stable v1 source/config/selected forest
  are rechecked; after removing the source, only remaining targets are checked. Earlier imports
  and removals survive failure, recursive removal errors expose uncertain extent, and reported
  logical bytes do not promise reclaimed disk space. Missing old sessions are retained memory,
  not automatic forgets; the UI keeps counts and at most ten gated labels of 256 characters for
  inspection. The native check/unlink and old-open-writer limits remain spec 7.5's cutover condition.

  All operations retain the existing single active/last receipt and connection Slot. Disconnect
  does not cancel; same-ID replay within that receipt does not repeat work. A lost/restarted
  receipt requires inspection and an explicit fresh operation, never automatic retry. Owner
  finalization, installation and cutover remain outside these synthetic acceptance tests.
  Cached receipt labels pass the current display rules again on delivery; unreadable rules hide
  labels while keeping IDs and effects. Loading saved settings clears preview/consent and cached
  display text, preserving unknown operations and counts until the next status GET. Late replies
  cannot restore discarded consent; a matching C2 status only refreshes an existing preview.

- **W6, first run and agents.** The first-run presets, the agents' wiring, the readiness checks, and
  the resident worker's and viewer's switches with resident.md's slice 2.

  W6's first inventory is an authenticated query-only GET, with seven independent native agent
  rows and fixed state codes. Finding a launcher or a matching registration does not prove login,
  permission to execute, or live delivery. Invalid oboete configuration does not hide agent facts.
  The reader shares native paths/builders and parsed-command predicates, opens no stores and
  launches no agent. The JA/EN panel preserves unsaved inputs and discards late reads; unknown
  response values remain unknown. Full Doctor checks, confirmed wiring and first-run completion
  remain subsequent W6 work. Both inventory and maintenance status reject nonempty raw queries;
  the existing caller's empty trailing question mark remains valid.
  Metadata failures and skipped Windows UNC/device namespaces produce unknown file flags, while
  a confirmed local launcher still produces found. Text reads stop at 1 MiB per file; larger
  files are unavailable and never parsed as a truncated registration. Windows also declines
  remote/unknown drives and reparses in any path component, using one native no-reparse open
  for metadata, content and canonical path observations. Unix dotfile symlinks remain supported.
  The native open starts from a trusted local drive-root handle; changing DOS drive mappings
  concurrently is outside that bootstrap guarantee. Explicit CLI actions retain local-link
  launchers; both passive Settings consumers use the guarded scan.
  A failed default-home path comparison leaves command alignment unknown; a confirmed missing
  default home retains the native custom-home behavior.

W1, W3, W5 and W6 are independent of each other once their backends exist; W2 is security scope and
W4 waits for the local embedder.
