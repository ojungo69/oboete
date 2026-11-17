# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`docs/spec.md` 0.1, "Owner decisions"**: the numbered decisions are this repository's ADRs.
  Read the ones that touch the area you are about to work in. `docs/spec-webui.md` (the page) and
  `docs/spec-client-runtime.md` (what comes after the cut-over) amend the spec where they say so.
- **`docs/spec.md` 0.2, "Short names and tags"**: the short names the spec and the docs use.
- **`GLOSSARY.md`** at the repo root, once one exists: the domain's terms.
- **`docs/code-layout.md`**, before adding a module, splitting a file or moving code.

If `GLOSSARY.md` does not exist, **proceed silently**. Don't flag its absence; don't suggest creating it
upfront. The `/domain-modeling` skill creates it lazily when terms actually get resolved.

## File structure

Single-context:

```
/
├── GLOSSARY.md          (once a term is resolved)
├── docs/
│   ├── spec.md          0.1: the numbered decisions; 0.2: short names
│   ├── spec-webui.md
│   ├── spec-client-runtime.md
│   └── code-layout.md
└── src/
```

There is no `docs/adr/`. A new decision is the next numbered entry in `docs/spec.md` 0.1, with its
date, written as what was decided. It is not a file of its own.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `GLOSSARY.md` and the spec. Don't drift to synonyms they explicitly avoid.

If the concept you need isn't defined yet, that's a signal: either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag decision conflicts

If your output contradicts a numbered decision, surface it explicitly rather than silently overriding:

> _Contradicts decision 54 (when a code PR merges), but worth reopening because…_
