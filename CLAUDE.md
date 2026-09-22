# oboete (Rust)

Lightweight single-binary memory for coding agents. Rewrite of the TypeScript `oboete-ts`; nothing from it is imported as code.

- **Spec of record**: `docs/plan.md` (what the owner wants, design decisions, milestones, M0 results). Read it before changing scope.
- **Scope discipline**: M1 = Claude Code / Codex / Grok Build capture, provider chain with fallback, redaction, FTS + vector search, MCP search, viewer, setup/doctor. Anything beyond M1 waits for the owner's one-week real-use verdict.
- **Process (owner-approved, lighter than the global default)**: spike/milestone notes in `docs/` instead of Spec Kit per PR; one review lane per PR: the Codex GitHub app reviews the PR when it opens and usually each push, but not always (PR #16 skipped one): after every push check that its summary comment names the new head SHA, else comment `@codex review`; answer each round (adopt, or reply why not) and merge when the run on the final head adds nothing (👍 on the PR); `cargo fmt --check`, `cargo clippy`, `cargo test` are the CI; bots: CodeQL only. GitGuardian's hits on the redaction test fixtures are test credentials (the owner skips them).
- **Implementation**: Claude Code writes the core directly; delegate to Codex/Grok only for independent, parallelizable pieces (agent adapters, donor-module ports).
- **Keys**: API keys live in the owner's `~/<NAME>_KEY.md` files, token on line 2. Never print them; never pass them to subprocess env (see `provider.rs`).
- **Measure, don't assume**: `oboete replay <fixture> --home <tmp>` prints hook latency, VmHWM, provider outcomes. Fixture of record: `../free-mem/test/fixtures/events-1000.jsonl`.
