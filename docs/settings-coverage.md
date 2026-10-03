# The settings page: coverage and the slices that close it (#94)

[spec-webui.md](spec-webui.md) asks for a coverage matrix as the closure evidence of #94: every
user setting and operation, where the page offers it, what it calls, when it takes effect and its
test. This is that matrix, kept with the work. It is not a second source of configuration: the
settings are config.toml's and the operations the CLI's, through the same typed backend
(`src/settings.rs`, one validation and one atomic, stale-checked write).

Owner, 2026-10-03: the settings page is complete at the switch (「設定画面は切り替え時に全部」), and
sync and the cloud come after it. So every row below is built before the switch, except those of
"Sync and devices", which follow milestone 6.

## The matrix

"Now" is main at c328d29. "—" is not on the page yet.

### Curation and spending

| Setting or operation | Spec | Now | Backend | Slice |
|---|---|---|---|---|
| A provider's order, on/off, daily budget, timeout, model | 1.4, 1.5 | page | `[chain]` | built |
| A provider's key, write-only | 6.6 | page (Linux) | `save_key` | built; macOS and Windows with #281 |
| Add, edit and remove a provider entry of a supported type | 1.4 | — | `[[providers]]` | W2 |
| Test a provider's connection, separately from a save | spec-webui | — | a typed probe | W2 |
| Curation on or off | 3.1, 7.4 | — | `[summary] curate` | W1 |
| Summary language, window size, idle wait | 3.1 | — | `[summary]` | W1 |
| The monthly cap of paid calls, with the month's spend beside it | 1.4, parity 13 | — | `paid_usd_per_month`, providers.db | W1 |
| Gemini's place in the chain | 1.4 | — | `gemini` | W1 |
| Resume a provider stopped for the owner | 1.4 | — | `oboete resume` | W1 |

### Delivery, capture and privacy

| Setting or operation | Spec | Now | Backend | Slice |
|---|---|---|---|---|
| Session start and per-prompt injection, on/off and size; correction notes | 4.4, 4.6, 4.8 | page | `[inject]` | built |
| Prompt storage, tool output detail | 2.3, 2.4 | page | `[capture]` | built |
| Exclude a repository from curation and embedding, and take it back | 5.5 | — | `oboete exclude` | W3 |
| Stop recording a repository or folder | 1.5, 6.1, parity 14 | — | capture exclusion (M5 slice 4) | W3 after M5 slice 4 |
| Additional redaction rules and allowed exceptions | 2.2, 6.4 | — | `[redaction]` | W3 |
| Raw retention | 6.2 | — | retention (M5 slice 3) | W3 after M5 slice 3 |
| Backup location | 2.6 | — | `[backup] dir` | W3 |
| Correct, mute, unmute a claim | 3.4, 6.1 | — | `oboete correct`, `mute` | W3 |
| A preference for every repository | 4.4 | — | `oboete pref` | W3 |
| Forget, with its preview | 6.2 | — | M5 | M5 slice 5 |

### Search and embeddings

| Setting or operation | Spec | Now | Backend | Slice |
|---|---|---|---|---|
| Embeddings: none, local or Workers AI, with caps | 4.10, 7.1 | — | `[embedding]` | W4 |
| The local model: source and size before the download, progress, readiness | spec-webui | — | local embedder (parity 5) | W4 after the local embedder |
| Index generations: building, active, rebuild after a change | 4.10 | — | `vec_generation` | W4 |

### First run, agents, import and maintenance

| Setting or operation | Spec | Now | Backend | Slice |
|---|---|---|---|---|
| First run: an unconfigured home, recommended presets | spec-webui | — | settings | W6 |
| Detect agents, wire and unwire their hooks | 7.2 | — | `oboete setup` | W6 |
| Readiness checks, as doctor reports them | 7.2 | — | `oboete doctor` | W6 |
| Import claude-mem's history, with its projects mapped to repositories | 7.4, parity N1 | — | `oboete import` | W5 after N1 |
| Migrate v1's store; import transcripts | 7.4 | — | `oboete migrate`, `transcript` | W5 |
| Recurate, with the list and estimate before the send | 1.7 | — | `oboete recurate` | W5 |
| Rebuild, restore | 1.7, 2.6 | — | `oboete rebuild`, `restore` | W5 |
| Update check and update | 7.3 | — | `oboete update` (not built) | W5 when the updater lands |
| The resident worker and viewer: on or off, a new page token | 1.8, 6.6 | — | `[worker]`, `[view]` | W6 with resident.md slice 2 |

### Sync and devices (after the switch, milestone 6)

| Setting or operation | Spec | Now | Backend | Slice |
|---|---|---|---|---|
| Deploy or connect the hub; add and remove devices; raw sync and its exclusions | 5 | — | milestone 6 | M6 |

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
- **W3, privacy and claims.** Exclusions, redaction rules and exceptions (validated as config.toml's
  are), the backup location, correct, mute and unmute, the global preference; then capture
  exclusion and retention as M5 builds them.
- **W4, embeddings.** The embedder's choice and caps, the local model's download with its consent and
  progress, and the generations' state; after the local embedder (parity 5).
- **W5, import and maintenance.** The imports with their previews, recurate with its estimate,
  rebuild and restore with their confirmations and progress; the updater's page when it lands.
- **W6, first run and agents.** The first-run presets, the agents' wiring, the readiness checks, and
  the resident worker's and viewer's switches with resident.md's slice 2.

W1, W3, W5 and W6 are independent of each other once their backends exist; W2 is security scope and
W4 waits for the local embedder.
