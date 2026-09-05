# @codemem/opencode-plugin

Persistent memory plugin for [OpenCode](https://opencode.ai).

## Install

Build the workspace and let the checkout CLI install a local managed wrapper:

```text
corepack pnpm run build
node packages/cli/dist/index.js setup --opencode-only
```

The pre-release setup writes a wrapper pinned to this package's built local entry
and removes managed npm plugin specs. Published or manually configured package
installs are not a supported free-mem runtime source.

## Documentation

- Repository: https://github.com/kunickiaj/codemem
- Full README: https://github.com/kunickiaj/codemem#readme
- User guide: https://github.com/kunickiaj/codemem/blob/main/docs/user-guide.md
- Architecture: https://github.com/kunickiaj/codemem/blob/main/docs/architecture.md
