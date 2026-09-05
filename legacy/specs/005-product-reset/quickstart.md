# Quickstart: Validate the Product Reset M0

Run from the Product Reset worktree root.

## 1. Confirm the isolated baseline

```bash
set -euo pipefail
git status --short --branch
cd vendor/codemem
corepack pnpm install --frozen-lockfile
corepack pnpm run build
CI=true corepack pnpm run check
cd ../..
```

Expected baseline before M0 documentation changes:

- build exits 0
- typecheck and lint exit 0
- 124 test files pass
- 1,895 tests pass and three remain marked todo

## 2. Verify Product Reset authority

```bash
set -euo pipefail
rg -Fn 'The **Product Reset M0** is the active work.' README.md
rg -Fn 'Active specification: [`specs/005-product-reset/spec.md`](specs/005-product-reset/spec.md)' README.md
rg -Fn 'The active product direction is the lightweight automatic-memory Product Reset:' evidence/README.md
rg -Fn '[`../specs/005-product-reset/spec.md`](../specs/005-product-reset/spec.md)' evidence/README.md
rg -Fn 'The v6 continuity documents, Rust-first ADR, continuity reference model, and broad capability rig' README.md
rg -Fn 'remain available as historical evidence. They are not active Product Alpha authority' README.md
rg -Fn '## Historical Evidence' evidence/README.md
rg -Fn -- '- `adr-003-rust-local-core.md` and `adr-005-rust-core-product-direction.md`' evidence/README.md
rg -Fn '### 2. Keep the current Codemem safety kernel' evidence/adr-006-product-reset.md
rg -Fn '### 3. Reject a claude-mem runtime fork; use it as a UX and test donor' evidence/adr-006-product-reset.md
if rg -ni '(v6 continuity|Rust-first|Verified Continuity).*(is|remains).*(active|current|canonical|required)' \
  README.md evidence/README.md; then
  exit 1
fi
if rg -n 'Active specification:.*(agent-memory-final-spec-v6|specs/00[1-4])' \
  README.md evidence/README.md; then
  exit 1
fi
```

Expected:

- README points to `specs/005-product-reset/spec.md` as active authority
- evidence index marks v6 continuity and Rust-first artifacts historical
- ADR records Codemem as the conditional base and rejects a claude-mem runtime fork

## 3. Prove M0 did not change runtime code

```bash
set -euo pipefail
git status --short
base=accaa29f5627c20c7e4c106a81211067fcf2bc42
git cat-file -e "${base}^{commit}"
git merge-base --is-ancestor "$base" HEAD
changed=$(git diff --name-only "$base" --)
printf '%s\n' "$changed"
untracked=$(git ls-files --others --exclude-standard)
if unexpected=$(printf '%s\n%s\n' "$changed" "$untracked" \
  | sort -u \
  | rg -v '^(README\.md|\.github/workflows/ci\.yml|evidence/(README\.md|adr-006-product-reset\.md)|specs/005-product-reset/.*\.(md|json|jq|mjs)|specs/005-product-reset/fixtures/artifacts/(candidate-example-v1|candidate-failure-example-v1)/candidate\.bundle)$'); then
  :
else
  status=$?
  test "$status" -eq 1
  unexpected=
fi
test -z "$unexpected"
while IFS= read -r path; do
  test -n "$path" || continue
  set +e
  check_output=$(git diff --no-index --check -- /dev/null "$path" 2>&1)
  status=$?
  set -e
  printf '%s' "$check_output"
  test "$status" -eq 0 || test "$status" -eq 1
  test -z "$check_output"
done <<< "$untracked"
git diff --quiet "$base" -- vendor/codemem harness
git diff --check "$base" HEAD --
git diff --check
git diff --cached --check
```

Expected: only root/evidence/specification documentation and Product Reset CI test wiring changed
from the pinned M0 base; untracked allowed artifacts plus base-to-HEAD, worktree, and index whitespace
checks pass.

## 4. Verify GitHub routing after the documentation commit is pushed

```bash
set -euo pipefail
export GH_REPO='ojungo69/free-mem'
test "$(gh repo view --json nameWithOwner --jq .nameWithOwner)" = "$GH_REPO"
gh pr view 133 --json state,mergedAt,title,url
gh pr view 133 --json state,mergedAt \
  | jq -e '.state == "CLOSED" and .mergedAt == null'
for n in 134 135; do
  gh issue view "$n" --json state | jq -e '.state == "CLOSED"'
done
for n in 136 137 138 139; do
  gh issue view "$n" --json number,state,labels
done | jq -s -e '
  (map({number, state, labels: ([.labels[].name] | sort)}) | sort_by(.number)) == [
    {"number":136,"state":"OPEN","labels":["area: product","enhancement","priority: p0","status: in progress","target: technical alpha"]},
    {"number":137,"state":"OPEN","labels":["area: adapter","area: product","area: storage","enhancement","priority: p0","status: ready for implementation","target: technical alpha"]},
    {"number":138,"state":"OPEN","labels":["area: product","area: retrieval","area: security","enhancement","priority: p1","status: blocked","target: technical alpha"]},
    {"number":139,"state":"OPEN","labels":["area: product","area: quality","area: release","enhancement","priority: p1","status: blocked","target: technical alpha"]}
  ]
'
gh issue list --state open --limit 200 --json number,labels \
  | jq -e '
    . as $issues
    | ($issues | length) == 12
    and ([
      .[]
      | {
          number,
          activeStatuses: ([
            .labels[].name
            | select(. == "status: in progress" or . == "status: ready for implementation")
          ] | sort)
        }
      | select(.activeStatuses | length > 0)
    ]
    | sort_by(.number))
    == [
      {"number":126,"activeStatuses":["status: ready for implementation"]},
      {"number":129,"activeStatuses":["status: ready for implementation"]},
      {"number":130,"activeStatuses":["status: ready for implementation"]},
      {"number":136,"activeStatuses":["status: in progress"]},
      {"number":137,"activeStatuses":["status: ready for implementation"]}
    ]
  '
# The snapshot covers the 69 pre-M0 issues. Replacement issues are checked above.
gh api --paginate "repos/$GH_REPO/issues?state=all&per_page=100" \
  | jq -s -e --slurpfile expected specs/005-product-reset/m0-post-mutation-issues.json '
      add
      | map(select(has("pull_request") | not))
      | ($expected[0] | map(.number)) as $numbers
      | ([.[]
          | select(.number as $number | ($numbers | index($number)) != null)
          | {number, state: (.state | ascii_upcase), labels: ([.labels[].name] | sort)}]
        | sort_by(.number)) == $expected[0]
    '
```

Expected:

- PR #133 is closed and unmerged
- #134 and #135 are closed as superseded
- one Product Reset parent and three child implementation issues exist
- the active-status mapping is exactly #126, #129, #130, #136, and #137 with the statuses above

## 5. Review future runtime contracts

Prerequisites for this step are Node.js 24 and `jq`. Validate the fixed Slice 1 fixture through its
canonical executable path. It always runs structural schema validation, digest reproduction, and
semantic jq validation in that order:

```bash
set -euo pipefail
node --experimental-strip-types \
  specs/005-product-reset/fixtures/validate-slice1-fixture.mjs
node --experimental-strip-types \
  specs/005-product-reset/contracts/validate-alpha-result.mjs
node --experimental-strip-types \
  specs/005-product-reset/contracts/validate-alpha-result.mjs \
  --runner-evidence specs/005-product-reset/fixtures/runner-evidence/alpha-runner-evidence-v1.failure-example.json \
  --runner-invocation-id candidate-failure-example-v1:fixture-invocation-v1 \
  --result specs/005-product-reset/fixtures/alpha-result-v1.failure-example.json
node --experimental-strip-types \
  specs/005-product-reset/contracts/validate-alpha-result.test.mjs
```

The result validator hashes each candidate file below
`fixtures/artifacts/<candidateId>/`; use `--artifact-root PATH` only when the same fixed artifacts
are staged under another runner-owned immutable root. Keep that exact snapshot mounted from
candidate execution through result validation.

- [Alpha comparison](contracts/alpha-comparison.md)
- [Effective capability manifest](contracts/capability-manifest.md)
- [InjectionPack](contracts/injection-pack.md)
- [Alpha result schema](contracts/alpha-result-v1.schema.json)
- [Alpha result semantic validator](contracts/alpha-result-v1.semantic.jq)
- [Alpha runner evidence schema](contracts/alpha-runner-evidence-v1.schema.json)
- [Alpha runner evidence validator](contracts/alpha-runner-evidence.mjs)
- [Alpha result lineage identity](contracts/alpha-result-lineage.mjs)
- [Alpha result canonical validator](contracts/validate-alpha-result.mjs)
- [Alpha result regression checks](contracts/validate-alpha-result.test.mjs)
- [Alpha result input and suite regression checks](contracts/validate-alpha-result-input.test.mjs)
- [Alpha result failed-record invariant checks](contracts/validate-alpha-result-failure.test.mjs)
- [Slice 1 fixed fixture](fixtures/slice1-bidirectional-en-v1.json)
- [Slice 1 example result](fixtures/alpha-result-v1.example.json)
- [Slice 1 failure example result](fixtures/alpha-result-v1.failure-example.json)
- [Slice 1 exact 16+1 suite regression corpus](fixtures/alpha-result-v1.suite-regression.json)
- [Slice 1 runner evidence examples](fixtures/runner-evidence/)
- [Slice 1 fixture schema](fixtures/slice1-bidirectional-en-v1.schema.json)
- [Slice 1 semantic validator](fixtures/slice1-bidirectional-en-v1.semantic.jq)
- [Slice 1 canonical validator](fixtures/validate-slice1-fixture.mjs)
- [M0 rollback](rollback.md)
- [M0 pre-mutation issue baseline](m0-pre-mutation-issues.json)
- [M0 post-mutation issue baseline](m0-post-mutation-issues.json)

These contracts guide later focused specs; M0 does not claim the runtime behaviors are implemented.

## Validation result — 2026-08-26T20:45:27+09:00

| Check | Result |
|---|---|
| `corepack pnpm install --frozen-lockfile` | PASS, exit 0 |
| `corepack pnpm run build` | PASS, exit 0 |
| `CI=true corepack pnpm run check` | PASS, exit 0; 124 test files and 1,895 tests passed, three todo |
| Product authority grep | PASS |
| Slice 1 fixture schema and semantic checks | PASS; positive fixture plus targeted schema, cross-host/downgrade transport, privacy, host-identity, output-limit recovery-manifest, environment, span, and profile mutations |
| Alpha result schema and semantic checks | PASS; eligible/non-eligible examples, secure runner-owned 16-scenario bundle plus required before-model negative result, full result-observation attestation, zero-report attack rejection, cross-mode-isolated cold reset receipts/data/process identities with pinned proof and first-measurement gaps, runner-bound host identity decisions, path-free opaque runner identities, retained warm generation, current invocation binding, immutable/non-overlapping evidence root, retrieval-before-selection presence/order, lifecycle-bound selection, fixed input counts, reachable quality failure and elapsed-budget deadline failure, source-bound unique trace identities, destination-sensitivity enforcement, ordered raw per-run timing, complete zero/nonzero render payloads, raw alias rejection, attempted/final pack ceilings, Slice 1 omission reasons, ordered attempted-item closure, forbidden-fact exclusion, durable revision identity, timeout-prefix event/persistence counts and provider-attempt egress, deadline-bound orphan sampling, retained completed selection, exact Agent-operation denominator, lifecycle-derived timeout/loss/conflict evidence, pre-terminal evidence nullability, inclusive timeout expiration, Slice 1 pack-failure refusal, per-resume egress evidence, raw output-limit receipt/observer evidence, runner-owned non-writable artifact traversal/bytes, 1 MiB file/stdin and runner-evidence inputs, FIFO rejection, timeout prefixes, and canonical exceptional-state mutations |
| Product Reset CI contract step | PASS locally; workflow `actionlint` and the committed regression command exit 0 |
| Rollback exact-state fence | PASS; 69-entry pre/post snapshot SHAs, live post-M0 fence, generated inverse state/full-label simulation, and empty parent sub-issue postcondition match |
| Local Markdown links (one-shot external validation) | PASS |
| `vendor/codemem/` and `harness/` diff | NONE |
| Base-to-HEAD, worktree, and index `git diff --check` | PASS |
| GitHub routing | PASS; 12 open issues, five active-status issues, PR #133 closed/unmerged |

Environment-specific deviations:

- `verify-tasks-report.md` is the immutable pre-mutation task-verification snapshot, not pre-M0 label
  authority. The `m0-pre-mutation-issues.json` and `m0-post-mutation-issues.json` pair is the exact
  69-issue rollback boundary; this table and `issue-routing.md` carry later local/live verification
  evidence without rewriting old verdicts.
- `pnpm install` reports that the vendored workspace has no nested `.git` directory when the
  Husky prepare script runs; prepare still exits 0 and the repository-level worktree remains the
  Git authority.
- Vitest intentionally emits failure-path setup, daemon-unavailable, and redaction fixture logs;
  the suite exits 0 with the counts above.
- The local Markdown link result was produced by a one-shot Node filesystem check during M0
  validation; no permanent link-checker dependency or script was added for this docs-only slice.
- CodeRabbit's final completed local review raised zero actionable issues. Its earlier valid
  suite-fixture separation was applied, while its repeated fixed-contract equal-time suggestion was
  rejected after fixture/source checks. The post-Codacy formatting retry reached the CLI rate limit;
  a fresh GitHub full review is requested after each pushed head.
- The latest GitHub Codex/CodeRabbit reviews raised timeout, input-boundary, loss-evidence, and exact
  rollback gaps; all valid findings were reproduced, fixed, and covered by focused cases. Cubic's
  earlier P1 contradicted the cited source values and the passing acceptance regression, so it was
  rejected. Its last completed post-fix review raised zero issues; the post-Codacy formatting retry
  returned an empty issue list with a timeout error. Grok's split
  contract/validator review returned `ok: true`; its full-diff, fixture/documentation, and final
  follow-up attempts timed out without session IDs, so those ranges are not claimed as Grok-reviewed.
  Ponytail found no unused definition, speculative abstraction, dependency, or removable
  compatibility layer.
- No functional validation command required a changed path, flag, retry, or skipped gate. A latest
  full Grok attempt timed out; its narrowed retry was stopped for host memory pressure after the
  earlier split contract review had returned `ok: true`.
