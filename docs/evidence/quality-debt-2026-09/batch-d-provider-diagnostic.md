# Batch D provider replay diagnostic

This run investigates the previously recorded SC-009 failure. It is **not** the paired resource
acceptance for the continuation, and it does not satisfy T032/T039.

- Source snapshot: `d724d5df65f6b7484f09d75dfa595c48fdc54c99`, before the additional concern splits.
- Engine SHA-256: `f8ea3316c6cbaf997ffd3f45cfadfa72e0f77a17e55c5b7b38b236601ed9ce4a`.
- Full raw output: [provider replay JSON](batch-d-provider-replay.json).
- Provider: Workers AI, `@cf/zai-org/glm-4.7-flash`; 79 calls, 1,818.6515 estimated neurons.
- Duration: 1,984.4 seconds; command exit 1.

The engine was unchanged. A fresh isolated `--home` copied the account's existing accepted
Workers AI configuration. A private Node preload loaded the existing environment file only when
the process was the exact measured bundle's `observe` command; hook, replay and unrelated-command
guards were checked separately. No account configuration, credential file, product code or
numeric bound changed. The preceding 18-event diagnostic made zero provider calls without this
preload and one applied provider call with it. The ordinary replay removes provider credentials.
Its Markdown text also hard-codes a credential-free run, so the JSON and this explicit provenance
are the evidence for this diagnostic.

| Measure | Observed | Result |
|---|---|---|
| Fact recall | Japanese 3/20, English 5/20, overall 8/40 (20%) | Below 90% |
| Capture | p99 212.1 ms; all 717 samples within 300 ms | Pass |
| Injection | p99 1,199.6 ms; 92.6% of 418 samples within 300 ms | Reported fail |
| Session start | ready max 1,254.7 ms; planned pending max 1,204.9 ms, 4/4 pending packs | Ready report fails |
| Worker RSS | 154,244 kB (150.6 MiB) | Above 150 MiB |
| Privacy, directives, duplicates | zero leaks/directive phrases/duplicate included memories | Pass |
| Lifecycle and hook exits | all tagged checks pass; all 1,143 hooks exit 0 | Pass |

The post-run database contained 14 applied, 32 fallback, 41 pending and 1 running batch; 37 ended
sessions still awaited a summary. The observe log recorded lease loss in 32 of 52 worker runs.
No process matching this diagnostic's exact bundle, `observe` command, account and `OBOETE_HOME`
remained when checked after the report. The running database row is not proof of a live process.

The harness waits only 45 seconds for summaries although a provider request may take 60 seconds;
it then proceeds silently. Before a held window it waits 500 ms for the lease and overwrites the
owner regardless of an in-flight worker. The final settle also overwrites the active lease.
Thus the JSON's completion is not evidence that the workload finished.

That incomplete tail alone does not explain recall. All 40 planted raw fact rows were eligible,
classified and unexcerpted, with terminal batches. Thirty belonged to applied provider batches:
6 hits, 2 memory-present misses and 22 absent from memories. Ten belonged to language-mismatch
fallbacks: 2 hits and 8 absent. Raw provider output is not retained, so the 22 absences cannot be
assigned specifically to model output or application handling from this run.

One scored miss, `f-en-06`, had already been delivered to the same conversation 4.3 seconds before
its matching prompt, where duplicate suppression correctly omitted it. Replay counts only the
current and session-start packs, so this is a false negative. `f-en-01` was available before its
query but omitted as `mmr_redundant`, a retrieval-quality miss. The database also recorded 29
pending session-start injections while the report labels only the four planned pending probes;
the worst supposed ready sample (Grok sequence 697) was actually pending.

A separately reviewed harness correction is needed for credential routing, terminal-state
barriers, readiness and already-delivered-memory accounting before a real-provider rerun can
serve as acceptance evidence. The observed retention/retrieval and RSS failures remain open.
No threshold or gate is waived by this diagnosis.
