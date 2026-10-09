# WebUI completion contract — owner amendment, 2026-10-02

## Status, authority and intent

This is a normative supplement to [spec.md](spec.md), tracked by [issue #94](https://github.com/ojungo69/oboete/issues/94). It records a requirement, not a claim that the UI is implemented. Read it with the base spec; for the user-facing configuration and management scope below, this later owner decision takes precedence over the older limited settings-page scope. All other owner decisions and safety requirements remain in force.

Owner request: 「webuiから簡単に全部設定したいんだけどそれは仕様に入ってる？」. After the limited current scope and the proposed WebUI-complete requirement were explained, the owner approved: 「変更して」.

**After the initial installation and launch, users must be able to complete setup, all user-configurable settings, and supported management operations through the local WebUI without typing shell commands or editing configuration/key files.** External-service login, issuing an API token at its provider, and unavoidable OS permission prompts are exceptions, not reasons to move oboete's own setup outside the UI. Explain the external step, provide its entry point, and let the user return, resume, and verify the result. A raw TOML editor or a command to copy is not completion.

This defines a simpler way to operate the existing product, not a new architecture or a requirement to adopt additional AI models. CLI use remains available for automation and recovery; it is no longer a prerequisite for ordinary setup and management after launch.

## What changes in the base spec

| Existing section or decision | Amendment |
|---|---|
| §1.5, §7.2 and A49: selected settings through the viewer, the rest through setup/config.toml | All supported user-facing settings and management operations require a UI path. Keep one configuration and one set of validation rules shared with the CLI. |
| §7.1 / §4.10: local BGE-M3 download and embedding selection through setup | Add a UI path for selection, approved model download, readiness and index-generation progress. The vector-space and search-fallback rules are unchanged. |
| §6.6 and #94's original no-key-input condition | Keys may be entered/replaced through a write-only UI path with secure backend storage. Saved values must never be returned to the browser. Linux-only support is an interim state, not completion on other release targets. |
| §7.2: agent setup, hub setup and imports through commands | Provide the equivalent guided UI operations as their existing backends land. External authorization remains explicit. |
| §7.3: only the update command initiates an update | An explicit UI action may invoke the same controlled updater. It must not bypass verification, locking, backup, migration, self-check or recovery. No automatic update or new package-manager path is introduced. |

The English-only CLI rule does not require an English-only WebUI. The setup/settings/management journey and its messages must support Japanese and English, extending #94's existing language-switch decision.

## Required coverage

| Area | Required user journey |
|---|---|
| First run and agents | Start with an unconfigured home; show recommended presets; detect/select supported agents; confirm and apply integration changes without losing existing configuration; run and explain the existing readiness checks. |
| Curation providers | Add, edit and remove entries for supported connection types; configure endpoint and model, order, on/off, timeout and permitted budgets; test the connection separately from saving. Do not reintroduce subscription daily caps contrary to owner decision 30. |
| Credentials | Enter and replace API keys, and see their registration/validation state. The backend supplies a safe owner-only storage location; normal users do not create a *_KEY.md file or choose filesystem permissions themselves. Existing key_file references remain compatible. |
| Embeddings and search | Choose none, local or Workers AI for supported model/backend combinations, including the planned local and Workers AI BGE-M3 choices. EmbeddingGemma 2 (#396) is a comparison candidate, not an adopted option; offer a model/backend pair only after it is supported. Approve downloads after seeing source and size; show progress, failure, saved/effective readiness, required re-index preview and active/building generations. Never mix incompatible vector spaces; preserve full-text fallback. |
| Capture, delivery and privacy | Configure injection on/off and size per kind, capture detail, repo/folder exclusion, additional redaction rules and permitted exceptions, raw retention and backup location. Preserve the existing correction, mute, withdraw and forget flows and their semantics. |
| Sync and devices | Configure or explicitly deploy/connect the existing optional hub, authorize/remove devices, select raw sync and exclusions, and inspect state, cost implications and failures. A local-only user must not need a cloud account. |
| Import and maintenance | Operate supported history import/migration, explicit recuration/re-indexing, diagnostics and update checks/updates; preview cost and destructive effects, confirm them, and show progress and safe recovery. |
| Additional admitted AI roles | When #336's pre-summary A or post-search B is implemented, expose independent supported configuration, effective state, fallback and applicable cost/egress consent. Offer only verified judge/reranker backend choices; TypeSafe AI Jev, Liquid AI d1, Clef and a dedicated reranker remain candidates rather than selected defaults. d1's choices include a local llama.cpp server beside Liquid's hosted API (owner, 2026-10-09): a loopback endpoint with a connection test and no key. B's relevance backend and any distinct evidence check need not be stacked on every search. An unavailable backing feature stays visibly unavailable; a page view or save does not call a model. |

This is coverage of accepted functionality, not a promise that arbitrary models, providers or protocols work. An unsupported combination must be explained, not silently accepted. Internal safety invariants and development/test-only knobs are not user settings to expose.

## Usability and behavior

- Plain Japanese/English descriptions explain each control's purpose, recommended/default value, and effect on cost or data leaving the device. Advanced options are progressively disclosed; users need not understand TOML or implementation names to complete a normal workflow.
- Maintain a coverage matrix with the implementing work: setting/operation, base-spec reference, UI location, shared backend or CLI equivalent, activation timing, OS support and acceptance test. It is the closure evidence for #94, not a second source of configuration truth.
- Show saved values, effective values and pending activation separately. Apply a safe reload through the UI when needed, without silent capture loss. A successful save alone does not imply that a provider is reachable, a model is ready, or an index is rebuilt.
- Validation, atomic persistence and stale-write protection are shared with the existing configuration path. Invalid input or a conflicting tab leaves the previous usable configuration intact. An unconfigured or invalid home has a safe guided setup/recovery path rather than a permanent instruction to edit a file.
- Connection tests are explicit. State the destination, test data and possible charge first; use a small synthetic fixture, not private history without separate consent. A page view or save must not silently start paid inference, a model download or cloud-resource creation.
- Long-running downloads, indexing, imports and updates show their state and bounded error details. Offer retry/resume/cancellation only where the operation is safe to do so; never interrupt an irreversible step merely to make a cancel button work.
- If a backing feature has not been implemented, show it accurately as unavailable. Hiding an unimplemented setting or printing a command does not satisfy the final coverage contract.

## Security and platform boundaries

Preserve the local loopback-only viewer, its token (per run; in a resident home one kept in an owner-only file, which carries every right of the page: spec 6.6, owner decision 37), Host/Origin checks, bounded requests, redaction/egress gates, budget controls, owner corrections and explicit destructive confirmations. This is not a remote dashboard, generic shell terminal, arbitrary-file editor, or a bypass for fixed decision/global-scope/deletion rules.

Key input is write-only: no saved key in GET responses, HTML, logs, errors or process arguments. Use platform-appropriate owner-only storage and existing safe-write protections. Carry #281 (macOS/Windows) and #285 (renameable ancestors) into the implementation rather than removing their checks. Where a location cannot meet the security contract, refuse it and provide a safe managed location; do not turn manual key-file creation into the normal fallback. Retain the existing provider credential boundaries; oboete does not collect third-party subscription session credentials.

Validate endpoint destinations and secret handling using the existing provider safety rules. Supporting a local model endpoint does not authorize unrestricted network probing, redirects that leak credentials, or arbitrary process execution. UI actions call typed operations, not user-supplied shell text.

## Delivery and acceptance

Implement incrementally on Design B. The base settings page remains work alongside milestones 3/4; model setup, privacy operations, sync and maintenance UI follow their backing milestones. Do not move every backend into milestone 4, discard finished work, or restart the project. #94 is the umbrella completion tracker and must not close after only the original narrow form is done.

For #336, the owner's October 8 order keeps W5C → W6 → local embedding/W4 → N1 → parity/M5 first, followed by A/B and their supported UI before the owner's PC cutover. The held model-evaluation conditions remain in force.

Before the corresponding functionality is declared complete, verify:

- [ ] A fresh user goes from initial launch to a configured agent and successful record/search round trip without hand-entered commands or config/key-file edits. Enumerate only unavoidable external steps, each with a return/resume path.
- [ ] Every supported user setting/operation has a coverage-matrix row, working UI and test; CLI and UI read the same effective values.
- [ ] Provider creation/model edits/reordering/on-off/key registration reach the next eligible run; invalid or concurrent saves preserve the previous configuration.
- [ ] Embedding selection, model acquisition and necessary generation rebuild are operable in the UI. Interrupted/failed acquisition or indexing preserves safe full-text operation and accurate status.
- [ ] Secret non-disclosure, owner-only storage and rejected unauthorized/cross-origin writes are tested on release targets. WSL/Linux, Windows native and the M1 Mac receive real-machine checks; CI-only targets retain their existing labels and evidence, not an unsupported completion claim.
- [ ] Each implemented sync/import/maintenance flow verifies explicit cost/egress/destructive consent, real progress and safe failure recovery. Update tests exercise the same verified updater as the CLI.
- [ ] A non-programmer can follow the Japanese setup and ordinary-change journey using the explanations and presets, without a TOML or programming tutorial.

A docs-only PR records this contract but does not satisfy these boxes or close #94. Keep implementation evidence in the existing milestone notes and linked PRs. The owner's running v1/claude-mem and real configuration, credentials and data are not changed by this amendment.
