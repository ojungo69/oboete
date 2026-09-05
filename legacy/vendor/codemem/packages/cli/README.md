# codemem

Persistent memory for AI coding agents — local-first SQLite storage, hybrid retrieval,
automatic OpenCode injection, and optional peer-to-peer sync.

This is the CLI workspace package vendored by free-mem. The current free-mem
runtime is pre-release and is not published; build and run this checkout directly.

## Install

```bash
corepack pnpm run build
node packages/cli/dist/index.js stats
```

## Quick commands

```bash
node packages/cli/dist/index.js --help
node packages/cli/dist/index.js setup --opencode-only
node packages/cli/dist/index.js stats
node packages/cli/dist/index.js search "query"
node packages/cli/dist/index.js distill --limit 10
node packages/cli/dist/index.js serve start
node packages/cli/dist/index.js mcp
```

## Documentation

- Repository: https://github.com/kunickiaj/codemem
- Full README: https://github.com/kunickiaj/codemem#readme
- User guide: https://github.com/kunickiaj/codemem/blob/main/docs/user-guide.md
- Architecture: https://github.com/kunickiaj/codemem/blob/main/docs/architecture.md
