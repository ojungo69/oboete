# Migration: Python to TypeScript backend

> **Status: Complete.** The TypeScript backend is the primary and only shipped runtime
> as of `0.20.x`. The legacy Python backend has been removed from `main`; use git
> history or the archive ref if you need the old implementation for reference.

## Background

codemem originally used a Python backend (`uvx codemem`). The TS backend
(`codemem` npm package) reached full feature parity and is now the sole shipped path.

## Database compatibility

Both backends share `~/.codemem/mem.sqlite`. The TS backend owns schema initialization
and migrations. The current sole-writer runtime directly upgrades schema 20 to schema 21;
schemas 6–19 remain readable by compatibility tooling but are not writable or restorable
by the schema-21 runtime.

For a schema 6–19 database, keep the verified backup, open it once with the last schema-20
runtime (the immediate pre-v21 revision `de08bbcb23a6a8c83783de927557446d338c5ac3`), verify
that it reaches schema 20, then retry with the current runtime. The schema-21 runtime fails
closed before backup activation or database mutation when this intermediate step is missing.

## Environment variables

| Variable | Purpose |
|---|---|
| `CODEMEM_DB` | Database path |
| `CODEMEM_DEVICE_ID` | Device identity |
| `CODEMEM_ACTOR_ID` | Actor identity |

## Migration history

| Stage | Status |
|---|---|
| Stage 1: Opt-in TS via `CODEMEM_RUNNER=npx` | Complete |
| Stage 2: TS as default runner | Complete |
| Stage 3: Python removal from release path | Complete (`0.20.x`) |
