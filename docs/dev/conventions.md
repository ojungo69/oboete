# Development conventions (M1 implementation)

Every implementer (Claude Code agents, Grok Build, Codex) reads this file before writing code. It
restates the decisions from `specs/007-oboete-m1-alpha/` (plan.md, research.md, data-model.md,
contracts/) that several modules share, so modules written in parallel agree without reading each
other. On any conflict the specification documents win over this file.

## Language, module system, build

- TypeScript 5.9, ESM only. `tsconfig.json`: `module` and `moduleResolution` `NodeNext`, `target`
  `ES2022`, `strict`, `verbatimModuleSyntax`, `isolatedModules`. Relative imports carry the `.js`
  extension: `import { openDatabase } from './db/open.js'`.
- Node >= 22.16 built-ins only where they exist: `node:sqlite` (`DatabaseSync`), `node:test`,
  `node:util` `parseArgs`, `crypto.randomUUID`, `Intl.Segmenter`, `worker_threads`. No Linux-only
  facility (no Unix sockets, `flock`, bash-only hooks); paths through `node:path` and `os.homedir()`.
- Build: `scripts/build.mjs` (security-owned, Claude Code only). `src/cli.ts` becomes
  `dist/oboete.mjs` (esbuild, one ESM file). Hook-path packages are bundled: `zod`, `smol-toml`,
  `@secretlint/core`, `@secretlint/secretlint-rule-preset-recommend`. Everything else (`ai`,
  `@ai-sdk/*`, `workers-ai-provider`, `hono`, `@hono/node-server`, `preact`) stays external and MUST
  be loaded with a dynamic `await import('...')` inside the command that needs it, never at the top
  level of a module the hook path loads (`capture`, `events`, `privacy/*`, `agents/*`, `db/open`,
  `config`, `paths`, `repo-identity`, `injection/*`, `retrieval/*`).
- `.sql` and `.md` files import as strings (`import sql0001 from './migrations/0001_core.sql'`).
  `OBOETE_VERSION` is a compile-time global declared in `src/globals.d.ts`.
- Named exports only (the Pi loader's default export in T051 is the single exception). Plain
  functions and plain objects; no class where a function does; no abstraction with one user.

## Tests

- `node:test` with `node:assert/strict`. Files: `test/unit/<module>.test.ts`,
  `test/migrations/*.test.ts`; shared helpers in `test/helpers/`. `npm test` compiles them to
  `build/test/**/*.test.mjs` and runs `node --test`. Tests pass on Node 22 and 24
  (`~/.nvm/versions/node/v22.23.1/bin/node` and `v24.16.0`).
- A test that touches storage creates a fresh directory with `fs.mkdtempSync` and points
  `OBOETE_HOME` at it; the real `~/.oboete` is never used. Use `test/helpers/home.ts` (T022) once
  it exists.
- Red first: write the failing test, run it, confirm it fails for the right reason, then implement.
  A test never recomputes its expected value through the code path it checks.

## Data directory and files

- `OBOETE_HOME`, else `~/.oboete`. Inside: `config.toml`, `memory.db` (plus `-wal` and `-shm`),
  `spool/`, `spool/pi-ack/`, `logs/hook.log`, `logs/observe.log`, and the `paused` marker file.
  `src/paths.ts` (T022) is the only module that composes these paths.
- Repository path rules live in `.oboete.toml` at the repository root (same TOML parser).

## Identifiers, hashes, time

- Time columns are Unix milliseconds (`Date.now()`).
- `sha256` = `node:crypto` `createHash('sha256')` over UTF-8, hex output. A composite key is
  hashed as `JSON.stringify([...parts])` so separators cannot collide.
- `raw_events.id`: sha256 of the most specific stable key (contracts/agents.md "Event identity"),
  computed by `src/events.ts` only.
- `material_hash` and `content_hash`: computed by `src/db/identity.ts` only.
  `memories.id` = `'m_' + content_hash.slice(0, 24)`.
- Every other id (`sessions.id`, `turns.id`, `observation_batches.id`, `injections.id`,
  `diagnostics.id`, reservation ids, Pi invocation ids): `crypto.randomUUID()`.
- Repository id: `src/repo-identity.ts` (T023), first 16 hex of sha256 over the normalized identity.

## Database access

- Open through `src/db/open.ts` (T019) only: `new DatabaseSync(path, { timeout })`,
  `PRAGMA journal_mode = WAL`, `PRAGMA foreign_keys = ON`; hook connections add
  `PRAGMA wal_autocheckpoint = 0`. Migrations are forward-only, one transaction each,
  `PRAGMA user_version` = the highest applied number.
- A read-then-write unit is one `BEGIN IMMEDIATE` transaction. Every worker write is fenced by
  `worker_lease.owner_token` (`... WHERE owner_token = ?`; zero rows changed means the lease was
  lost) except the exhaustion signal in `provider_usage`.
- SQL is never assembled from untrusted strings; values go through parameters of `db.prepare()`.

## Sensitivity and egress (never re-implemented locally)

- Sensitivity values: `local_only` (default), `eligible`, `secret`, `private`; lattice
  `secret > private > local_only > eligible` (stricter wins).
- One function evaluates `destination_rules` (`src/privacy/egress.ts`, T029) and one shared query
  (`src/db/queries.ts`, T021) applies repository scope, sensitivity, `review_state`, tombstone and
  validity for injection, CLI, MCP and the viewer. No caller writes its own `WHERE sensitivity ...`.
- The producing agent is provenance only: it never enters a hash, a provider request, a fallback
  body, or a pack.

## CLI and processes

- `src/cli.ts` dispatches with `node:util` `parseArgs`. A command module exports
  `run<Command>(argv: string[]): Promise<number>` and returns the exit code of contracts/cli.md
  (`0`, `1`, `2`, `3`). Agent-invoked commands (`hook`, `capture`, `inject`) always return `0` and
  print nothing but the pack to stdout.
- Logs: one line per entry, `<ISO time> <level> <message> key=value ...`, appended to the file
  under `logs/`; never a credential value, never redacted content.
- The detached worker is spawned as
  `spawn(process.execPath, [bundlePath, 'observe'], { detached: true, stdio: 'ignore' }).unref()`.

## Style

- Small files, plain functions, no speculative options or configuration. User-facing text (CLI
  output, doctor items, packs) is written in full sentences without abbreviations.
- Comments only where the reason is not visible in the code, citing the requirement or research
  item (`// FR-018: redact before the first write`). A deliberate simplification with a known
  ceiling carries `// ponytail: <ceiling>, <upgrade path>`.
