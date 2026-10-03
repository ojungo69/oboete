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

"Now" is main at c328d29. "Takes effect" is when a saved value is used: the worker reads its
settings again for each window it curates, a hook reads its own at each event, and a key file is
read at each call. "OS" is where the row works; "all" is WSL/Linux, macOS and Windows, with the
real-machine checks spec-webui.md asks for at the end. "Test" names the test that proves the row,
in `src/` unless a file is named; a slice fills in its rows' tests, and #94 closes when no row is
left with "—" there.

### Curation and spending

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| A provider's order, on/off, daily budget, timeout, model | 1.4, 1.5 | page | `[chain]` | the next window | all | `settings.rs` `a_save_writes_only_what_changes`, `a_refused_save_leaves_the_file`, `a_model_the_entry_cannot_price_is_refused`; `view.rs` `a_settings_save_passes_every_guard_first` | built |
| A provider's key, write-only | 6.6 | page | `save_key` | the next call | Linux | `settings.rs` `a_key_saved_from_the_page_reaches_its_file_and_no_answer`, `no_key_is_shown_or_taken`; `view.rs` `a_key_save_answers_without_the_key` | built |
| A provider's key on macOS and Windows, in owner-only storage there | 6.6, spec-webui | refused (`a_key_save_waits_for_owner_only_files_off_linux`) | `save_key` | the next call | macOS, Windows | — | #281, after the switch, before #94 closes |
| Add, edit and remove a provider entry of a supported type | 1.4 | — | `[[providers]]` | the next window | all | — | W2 |
| Test a provider's connection, separately from a save | spec-webui | — | a typed probe | at once, nothing saved | all | — | W2 |
| Curation on or off | 3.1, 7.4 | — | `[summary] curate` | the next window | all | — | W1 |
| Summary language, window size, idle wait | 3.1 | — | `[summary]` | the next window | all | — | W1 |
| The monthly cap of paid calls, with the month's spend beside it | 1.4, parity 13 | — | `paid_usd_per_month`, providers.db | the next call | all | — | W1 |
| Gemini's place in the chain | 1.4 | — | `gemini` | the next window | all | — | W1 |
| Resume a provider stopped for the owner | 1.4 | — | `oboete resume` | the worker's next pass | all | — | W1 |

### Delivery, capture and privacy

| Setting or operation | Spec | Now | Backend | Takes effect | OS | Test | Slice |
|---|---|---|---|---|---|---|---|
| Session start and per-prompt injection, on/off and size; correction notes | 4.4, 4.6, 4.8 | page | `[inject]` | the next session start or prompt | all | `settings.rs` `inject_settings_check_ranges_and_save_alone` | built |
| The terminal line at session start, on/off | parity 9 | — | `[inject] session_start_note` | the next session start | all | — | W3 |
| Prompt storage, tool output detail | 2.3, 2.4 | page | `[capture]` | the next record | all | `settings.rs` `a_save_writes_only_what_changes`, `a_refused_save_leaves_the_file` | built |
| Exclude a repository from curation and embedding, and take it back | 5.5 | — | `oboete exclude` | the next window | all | — | W3 |
| Stop recording a repository or folder | 1.5, 6.1, parity 14 | — | capture exclusion (M5 slice 4) | the next record | all | — | W3 after M5 slice 4 |
| Additional redaction rules and allowed exceptions | 2.2, 6.4 | — | `[redaction]` | the next record and the next send; a rule added rescans what is stored, its hits becoming range tombstones (2.2), with the rescan's state on the page | all | — | W3 |
| Raw retention | 6.2 | — | retention (M5 slice 3) | the next retention pass | all | — | W3 after M5 slice 3 |
| Backup location | 2.6 | — | `[backup] dir` | the next backup | all | — | W3 |
| Correct, mute, unmute a claim | 3.4, 6.1 | — | `oboete correct`, `mute` | at once | all | — | W3 |
| A preference for every repository | 4.4 | — | `oboete pref` | the next session start | all | — | W3 |
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
| First run: an unconfigured home, recommended presets | spec-webui | — | settings | at once | all | — | W6 |
| Detect agents, wire and unwire their hooks | 7.2 | — | `oboete setup` | the agent's next session | all | — | W6 |
| Readiness checks, as doctor reports them | 7.2 | — | `oboete doctor` | at once, read only | all | — | W6 |
| Import claude-mem's history, with its projects mapped to repositories | 7.4, parity N1 | — | `oboete import` | when confirmed | all | — | W5 after N1 |
| Migrate v1's store; import transcripts | 7.4 | — | `oboete migrate`, `transcript` | when confirmed | all | — | W5 |
| Recurate, with the list and estimate before the send | 1.7 | — | `oboete recurate` | when confirmed | all | — | W5 |
| Rebuild, restore | 1.7, 2.6 | — | `oboete rebuild`, `restore` | when confirmed | all | — | W5 |
| Update check and update | 7.3 | — | `oboete update` (milestone 7) | when confirmed | all | — | after the switch, with milestone 7; W5 shows it as not built |
| The resident worker and viewer: on or off, a new page token | 1.8, 6.6 | — | `[worker]`, `[view]` | the worker when it next goes idle, the viewer at its next start; a new token at once | all; the token file Linux, macOS and Windows with #281 | — | W6 with resident.md slice 2 |

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
- **W3, privacy and claims.** The terminal line at session start, exclusions, redaction rules and
  exceptions (validated as config.toml's are, with the rescan an added rule starts and its state),
  the backup location, correct, mute and unmute, the global preference; then capture exclusion and
  retention as M5 builds them.
- **W4, embeddings.** The embedder's choice and caps, Workers AI's account id and write-only token, the
  local model's download with its consent and progress, and the generations' state; after the local
  embedder (parity 5).
- **W5, import and maintenance.** The imports with their previews, recurate with its estimate,
  rebuild and restore with their confirmations and progress; the updater's row shown as not built
  until milestone 7 builds it.
- **W6, first run and agents.** The first-run presets, the agents' wiring, the readiness checks, and
  the resident worker's and viewer's switches with resident.md's slice 2.

W1, W3, W5 and W6 are independent of each other once their backends exist; W2 is security scope and
W4 waits for the local embedder.
