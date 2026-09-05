# oboete

> **Rewrite in progress.** oboete (覚えて, "remember") is the successor to free-mem. The previous
> implementation is preserved under [`legacy/`](legacy/README.md) as read-only evidence. There is no
> supported package yet.

**Automatic, shared memory for Claude Code, Codex, Grok Build, and Pi, without a subscription.**

oboete captures what happens in a coding session, summarizes it in the background, and injects the
relevant decisions, discoveries, and next actions into the next session of any supported agent. The
target is functional parity with claude-mem and cmem pro, running on the user's own machine and
free-tier services.

## Shape

- One SQLite file, `~/.oboete/memory.db`, is the product. No resident daemon, RPC, or port.
- Hooks are short-lived processes with a 300 ms budget; summarization runs in a detached worker.
- All four agents share one store. Boundaries are sensitivity and repository, never the agent.
- Sensitivity is decided at capture and fails closed; availability fails open. Secrets are
  redacted before storage.
- Observer LLM: Cloudflare Workers AI free tier by default, any OpenAI-compatible endpoint
  otherwise, and a rule-based fallback when neither is reachable.
- TypeScript on Node.js >= 22.16 with `node:sqlite`; the schema and CLI contract are the seam for
  any later rewrite.

The full set of rules is in [`CONSTITUTION.md`](CONSTITUTION.md).

## Milestones

| Milestone | Scope |
| --- | --- |
| M1 | Self-use Alpha: four agents, web viewer, doctor, isolated dogfood |
| M2 | Encrypted R2 sync, hybrid lexical + vector search |
| M3 | npm publication |
| M4 | macOS, private MCP link |
| M5 | Windows |

## Layout

- `CONSTITUTION.md` — project principles and constraints (authoritative).
- `docs/research/` — verified third-party contracts (hook payloads, provider APIs); created with the M1 specification.
- `specs/` — Spec Kit features for oboete milestones (created per milestone).
- `scripts/` — repository gates (DCO checker).
- `legacy/` — the free-mem era, read-only.

## License

Apache-2.0. See [`LICENSE`](LICENSE), [`NOTICE`](NOTICE), and [`CONTRIBUTING.md`](CONTRIBUTING.md).
