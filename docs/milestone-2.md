# Milestone 2: Record

The plan is docs/milestone-2-plan.md. The spec is docs/spec.md sections 1-2, 4.9 and 8.4 item 2. Acceptance tests carried from the plan's review are in issue #83.

## The `v1` branch (Task 0, 2026-09-26)

- `v1` was branched from `main` at 431bafc. Its `src/`, `Cargo.toml` and `Cargo.lock` are the same as at b95b827, the commit of the owner's last install (#71): `git diff --stat b95b827 431bafc -- src Cargo.toml Cargo.lock` prints nothing.
- `v1` is protected by the same ruleset as `main`: no deletion or force push, changes by pull request, linear history, required checks.
- Until the cut-over after milestone 5 (spec 7.5), the owner's binary is built from `v1`. A v1 fix (real loss or safety only) is a PR into `v1`. `main` is Design B from Task 1 on.
- Design B's hooks in the dogfood user (Task 0 step 4) are set up once Task 2 gives `main` a hook path of its own. Until then `main` builds the same binary as `v1`.
