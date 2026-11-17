# Dependencies: how they are kept current

Decision 113 (2026-11-17). oboete's dependencies are the crates in `Cargo.lock` and the GitHub
Actions its workflows use. Dependabot keeps both current (`.github/dependabot.yml`):

- Every Monday it opens pull requests for the crates in `Cargo.lock`, the dependencies of the
  dependencies too, and for the Actions the workflows pin by commit.
- No `cooldown` is set: a crate's release is proposed after Dependabot's own default wait (three
  days in GitHub's documentation), and each update is read before it is merged.
- Minor and patch releases of the crates come together in one pull request a week. A release
  that may break the build (a new major, or a new minor of a `0.x` crate) comes on its own, and its
  code changes are made in that pull request or in one that replaces it.
- Security updates (Dependabot alerts and security updates, on in the repository's settings) are
  opened when an advisory is published, with no wait.

Every one of these pull requests goes through the review lanes of any other (CLAUDE.md, Process)
and is merged by hand. None is set to merge by itself: a dependency someone took over would
otherwise reach main, and then the next install, without anyone reading it.

## What is updated by hand

- `fastembed` and `ort` (`=7.1.1`, `=2.0.0-rc.13` in `Cargo.toml`): the `local-embed` feature
  loads the ONNX Runtime library at run time, and these two are pinned to what that library and
  bge-m3 were checked with. Dependabot leaves them alone. They are updated together, with a
  `--features local-embed` build run on a machine that embeds.
- A release that needs a newer Rust than `rust-version` in `Cargo.toml` (1.95): cargo's resolver
  picks versions the minimum Rust builds, and the `msrv` job fails on one that does not. Raising
  `rust-version` is a change of its own.

## 2026-11-17

`cargo update` within the versions `Cargo.toml` allows moved 42 crates to their newest releases,
one of them new (`utf_types`). Dependabot showed no open alert before the update.
