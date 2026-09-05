# Contributing to oboete

Thanks for taking the time to look at this project. Please read this page before opening a pull
request — a few of the rules here are unusual, and they exist because of how this repository is built.

## License of your contribution

oboete is licensed under the Apache License 2.0 (see [`LICENSE`](LICENSE)). Contributions are
accepted under the same license — **inbound = outbound**. You keep the copyright in what you write;
we do not ask you to assign it, and there is no CLA.

The rationale for the license choice, the dependency license scan behind it, and the
material-by-material breakdown are recorded in
[`legacy/evidence/adr-004-licensing.md`](legacy/evidence/adr-004-licensing.md).

## Sign your commits (DCO)

Every commit must carry a `Signed-off-by:` line, which certifies that you have the right to submit
the work under the project's license. Git adds it for you:

```bash
git commit -s -m "your message"
```

The `dco` check runs on every pull request targeting `main`, and checks the commits from the merge
base with `main` through the pull request head. A pull request into some other branch is checked when
its work reaches `main`. A commit passes when a `Signed-off-by: Name <email>` trailer
matches its author or committer email (case-insensitively).

There are no exemptions, including for bots. An author email alone is not proof of who made a commit
— anyone can pass `git commit --author` — so exempting an address would let any contributor claim it.
Pull requests opened by Dependabot or another bot are therefore checked like everyone else: if their
commits are not signed off, land the change on your own branch with `git commit -s` instead of
merging the bot's branch.

This repository squash-merges pull requests, so the generated commit on `main` does not retain each
trailer. The sign-off record remains on the pull request commits, which can be reached from the
squash commit's `(#N)` reference. Commits already on `main` before this check was introduced are
grandfathered and are not checked retroactively.

When Claude Code or Codex CLI creates a commit, include `git commit -s` in its instructions; agent
commits are checked in the same way.

The check runs from the target branch, not from the pull request: both the workflow
(`.github/workflows/dco.yml`) and the checker (`scripts/dco-check.mjs`) are read from `main`, and the
pull request head is only read as git history. A pull request that edits either file is still checked
by the version already on `main`, so it cannot weaken the checker that gates it.

Adding a second job named `dco` elsewhere does not help either: a required check is satisfied only when
every check run carrying that name passed, so the trusted job's failure still stands. `dco` is a
required status check on `main`, so a pull request whose commits are not signed off cannot be merged.

The full text you are certifying is the [Developer Certificate of Origin 1.1](https://developercertificate.org/).

## Declaring AI-assisted work

Much of this repository was written with AI coding agents, so AI assistance is expected rather than
discouraged. What we ask is that it be visible: if an agent wrote or substantially shaped your patch,
say so in the pull request, name the tool, and confirm you have reviewed the output and that it does
not reproduce third-party code.

## Bringing in third-party code

Do not paste code from another project into this repository without recording where it came from.
If your change vendors, copies, or adapts third-party material:

1. Record the upstream URL, the exact commit, and the license in the same pull request.
2. Add or update the entry in `THIRD_PARTY_NOTICES.md` (create it with the first entry).
3. Keep the upstream license file intact. Do not add oboete headers to vendored files.

`legacy/vendor/codemem/` is a pinned MIT snapshot of an upstream project from the free-mem era. It is
not built, tested, or changed; see [`legacy/README.md`](legacy/README.md).

## Never put real data in fixtures

Test fixtures, capability captures, benchmark inputs, and issue reports must not contain real
credentials, private memory content, or local filesystem paths from your machine. Use synthetic
repositories and isolated HOME/config directories. A fixture that encodes a machine-specific layout
will be rejected even if the test passes.

## Before you open a pull request

- Run `node --test scripts/dco-check.test.mjs`, plus any check that covers what you touched. The `npm ci && npm run build && npm test` step arrives together with `package.json` in the first M1 implementation pull request.
- Never edit generated files by hand; change the source and regenerate.
- If you change the SQLite schema, the CLI contract, or a hook payload, update the matching
  specification under `specs/` in the same pull request. Those documents describe what the
  implementation actually does, and a silent drift between them is treated as a defect.
- Do not weaken a CI gate to make a check pass. If a gate is wrong, say so in the pull request and
  fix the gate deliberately, in its own change.
- State whether the change complies with Principles I-VI of [`CONSTITUTION.md`](CONSTITUTION.md),
  and name any approved exception.

## Security issues

Do not open a public issue for a vulnerability. Follow [`SECURITY.md`](SECURITY.md).
