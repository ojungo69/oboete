# Rollback: Product Reset M0 GitHub State

Use this runbook only if the Product Reset documentation pull request is abandoned or reverted on
the default branch. Do not run it merely because a later implementation slice changes scope.

## Preconditions

1. Confirm the Product Reset branch will not become default-branch authority, or confirm its merge
   commit has been reverted.
2. Capture current issue, label, parent/sub-issue, blocked-by, and pull-request state before any
   mutation.
3. Announce the rollback on #136 so users do not follow child issues during the transition.
4. Run mutations serially with `set -e`; after any failure, re-query live state before resuming.
5. Use GitHub CLI 2.94.0 or newer and verify that `gh issue edit` exposes `--remove-parent` and
   `--remove-blocked-by` before the first mutation.

## Read-only preview

The 69-issue original universe is the disjoint union of eight kept issues and 61 issues reopened by
rollback. The four replacement issues are tracked separately and are not part of that original set.

```bash
set -euo pipefail
export GH_REPO='ojungo69/free-mem'
test "$(gh repo view --json nameWithOwner --jq .nameWithOwner)" = "$GH_REPO"
export RESET_ROLLBACK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/free-mem-reset-rollback.XXXXXX")
export RESET_PRE_M0_ISSUES='specs/005-product-reset/m0-pre-mutation-issues.json'
export RESET_POST_M0_ISSUES='specs/005-product-reset/m0-post-mutation-issues.json'
export RESET_ORIGINAL_ISSUES=$(jq -c '[.[].number]' "$RESET_PRE_M0_ISSUES")
export RESET_REPLACEMENT_ISSUES='[136,137,138,139]'
chmod 700 "$RESET_ROLLBACK_DIR"

test -f "$RESET_PRE_M0_ISSUES"
test -f "$RESET_POST_M0_ISSUES"
test "$(sha256sum "$RESET_PRE_M0_ISSUES" | cut -d ' ' -f 1)" = \
  ba5600b60f829cc1081a0920e57d2883f566c105c47c70df28d72c12745f600e
test "$(sha256sum "$RESET_POST_M0_ISSUES" | cut -d ' ' -f 1)" = \
  a1e888398f18c0731db7545277e381c7637c8c3d11b394703b380acb51e3d47a

gh issue list --state all --limit 200 --json number,state,labels \
  > "$RESET_ROLLBACK_DIR/issues-before.json"
gh pr view 131 --json number,state,mergedAt,headRefName \
  > "$RESET_ROLLBACK_DIR/pr-131-before.json"
gh pr view 133 --json number,state,mergedAt,headRefName \
  > "$RESET_ROLLBACK_DIR/pr-133-before.json"
gh api graphql -f query='query { repository(owner:"ojungo69", name:"free-mem") {
  i126: issue(number:126) { number parent { number } }
  i129: issue(number:129) { number parent { number } }
  i130: issue(number:130) { number parent { number } }
  i136: issue(number:136) { number state subIssues(first:20) { nodes { number state } } }
  i137: issue(number:137) { number state parent { number } blockedBy(first:20) { nodes { number } } }
  i138: issue(number:138) { number state parent { number } blockedBy(first:20) { nodes { number } } }
  i139: issue(number:139) { number state parent { number } blockedBy(first:20) { nodes { number } } }
} }' > "$RESET_ROLLBACK_DIR/relationships-before.json"

test -s "$RESET_ROLLBACK_DIR/issues-before.json"
test -s "$RESET_ROLLBACK_DIR/pr-131-before.json"
test -s "$RESET_ROLLBACK_DIR/pr-133-before.json"
test -s "$RESET_ROLLBACK_DIR/relationships-before.json"
jq -e '.state == "OPEN" and .mergedAt == null' \
  "$RESET_ROLLBACK_DIR/pr-131-before.json"

for snapshot in "$RESET_PRE_M0_ISSUES" "$RESET_POST_M0_ISSUES"; do
  jq -e --argjson original "$RESET_ORIGINAL_ISSUES" '
    length == 69
    and ([.[].number] | sort) == ($original | sort)
    and ([.[].number] | length) == ([.[].number] | unique | length)
    and all(.[]; .state == "OPEN" or .state == "CLOSED")
    and all(.[]; .labels == (.labels | unique | sort))
  ' "$snapshot"
done
jq -e 'all(.[]; .state == "OPEN")' "$RESET_PRE_M0_ISSUES"

jq -S . "$RESET_POST_M0_ISSUES" \
  > "$RESET_ROLLBACK_DIR/issues-post-m0.expected.json"
jq -S --argjson original "$RESET_ORIGINAL_ISSUES" '
  [.[]
    | select(.number as $n | ($original | index($n)) != null)
    | {number, state, labels: ([.labels[].name] | sort)}]
  | sort_by(.number)
' "$RESET_ROLLBACK_DIR/issues-before.json" \
  > "$RESET_ROLLBACK_DIR/issues-post-m0.actual.json"
diff -u \
  "$RESET_ROLLBACK_DIR/issues-post-m0.expected.json" \
  "$RESET_ROLLBACK_DIR/issues-post-m0.actual.json"

jq -n -S \
  --slurpfile pre "$RESET_PRE_M0_ISSUES" \
  --slurpfile post "$RESET_POST_M0_ISSUES" '
  [$post[0][] as $after
    | $pre[0][]
    | select(.number == $after.number)
    | {
        number,
        remove: ($after.labels - .labels),
        add: (.labels - $after.labels)
      }
    | select((.remove | length) > 0 or (.add | length) > 0)]
' > "$RESET_ROLLBACK_DIR/issue-label-rollback.json"
jq -e '
  all(.[];
    (.remove | length) == (.remove | unique | length)
    and (.add | length) == (.add | unique | length)
      and ([.remove[], .add[]] | length) == ([.remove[], .add[]] | unique | length))
' "$RESET_ROLLBACK_DIR/issue-label-rollback.json"
jq -n -S \
  --slurpfile post "$RESET_POST_M0_ISSUES" \
  --slurpfile operations "$RESET_ROLLBACK_DIR/issue-label-rollback.json" '
  ($operations[0] | map({key: (.number | tostring), value: .}) | from_entries) as $by_number
  | $post[0]
  | map(.number as $n
    | ($by_number[($n | tostring)] // {remove: [], add: []}) as $operation
    | .state = "OPEN"
    | .labels = ((.labels - $operation.remove) + $operation.add | unique | sort))
  | sort_by(.number)
' > "$RESET_ROLLBACK_DIR/issues-rollback.simulated.json"
jq -S . "$RESET_PRE_M0_ISSUES" \
  > "$RESET_ROLLBACK_DIR/issues-pre-m0.expected.json"
diff -u \
  "$RESET_ROLLBACK_DIR/issues-pre-m0.expected.json" \
  "$RESET_ROLLBACK_DIR/issues-rollback.simulated.json"

jq -e --argjson replacements "$RESET_REPLACEMENT_ISSUES" '
  ([.[] | select(.number as $n | $replacements | index($n)) | .number] | sort)
    == ($replacements | sort)
  and all(.[] | select(.number as $n | $replacements | index($n)); .state == "OPEN")
' "$RESET_ROLLBACK_DIR/issues-before.json"
jq -e '.state == "CLOSED" and .mergedAt == null' \
  "$RESET_ROLLBACK_DIR/pr-133-before.json"
jq -e '
  .data.repository as $r
  | ($r.i126.parent.number == 137 and $r.i129.parent.number == 137
    and $r.i130.parent.number == 137)
  and ([ $r.i136.subIssues.nodes[].number ] | sort) == [137,138,139]
  and ($r.i137.parent.number == 136 and $r.i138.parent.number == 136
    and $r.i139.parent.number == 136)
  and ($r.i137.blockedBy.nodes | length) == 0
  and ([ $r.i138.blockedBy.nodes[].number ] | sort) == [137]
  and ([ $r.i139.blockedBy.nodes[].number ] | sort) == [137,138]
' "$RESET_ROLLBACK_DIR/relationships-before.json"
printf 'Snapshot: %s\n' "$RESET_ROLLBACK_DIR"
```

Keep the same shell and `RESET_ROLLBACK_DIR` value through post-verification. Compare the snapshot
with `issue-routing.md`. `issues-before.json` is the live post-M0 rollback input; the committed
pre/post snapshot pair is the exact state/full-label authority and generates every inverse label
operation. Do not substitute the live snapshot for either committed boundary.

## Restore the former issue set

The M0 closure set is exact and contains 61 issues.

```bash
set -euo pipefail
test -n "${RESET_ROLLBACK_DIR:-}"
GH_VERSION=$(gh version | sed -n '1s/^gh version \([^ ]*\).*/\1/p')
test -n "$GH_VERSION"
test "$(printf '%s\n' 2.94.0 "$GH_VERSION" | sort -V | head -n 1)" = 2.94.0
GH_ISSUE_EDIT_HELP=$(gh issue edit --help)
printf '%s\n' "$GH_ISSUE_EDIT_HELP" | rg -q -- '--remove-parent'
printf '%s\n' "$GH_ISSUE_EDIT_HELP" | rg -q -- '--remove-blocked-by'

gh issue list --state all --limit 200 --json number,state,labels \
  > "$RESET_ROLLBACK_DIR/issues-pre-mutation.json"
gh pr view 131 --json number,state,mergedAt,headRefName \
  > "$RESET_ROLLBACK_DIR/pr-131-pre-mutation.json"
gh pr view 133 --json number,state,mergedAt,headRefName \
  > "$RESET_ROLLBACK_DIR/pr-133-pre-mutation.json"
gh api graphql -f query='query { repository(owner:"ojungo69", name:"free-mem") {
  i126: issue(number:126) { number parent { number } }
  i129: issue(number:129) { number parent { number } }
  i130: issue(number:130) { number parent { number } }
  i136: issue(number:136) { number state subIssues(first:20) { nodes { number state } } }
  i137: issue(number:137) { number state parent { number } blockedBy(first:20) { nodes { number } } }
  i138: issue(number:138) { number state parent { number } blockedBy(first:20) { nodes { number } } }
  i139: issue(number:139) { number state parent { number } blockedBy(first:20) { nodes { number } } }
} }' > "$RESET_ROLLBACK_DIR/relationships-pre-mutation.json"

jq -S 'map(.labels |= sort_by(.name)) | sort_by(.number)' \
  "$RESET_ROLLBACK_DIR/issues-before.json" \
  > "$RESET_ROLLBACK_DIR/issues-before.lock.json"
jq -S 'map(.labels |= sort_by(.name)) | sort_by(.number)' \
  "$RESET_ROLLBACK_DIR/issues-pre-mutation.json" \
  > "$RESET_ROLLBACK_DIR/issues-pre-mutation.lock.json"
diff -u \
  "$RESET_ROLLBACK_DIR/issues-before.lock.json" \
  "$RESET_ROLLBACK_DIR/issues-pre-mutation.lock.json"
for stem in pr-131 pr-133 relationships; do
  jq -S . "$RESET_ROLLBACK_DIR/$stem-before.json" \
    > "$RESET_ROLLBACK_DIR/$stem-before.lock.json"
  jq -S . "$RESET_ROLLBACK_DIR/$stem-pre-mutation.json" \
    > "$RESET_ROLLBACK_DIR/$stem-pre-mutation.lock.json"
  diff -u \
    "$RESET_ROLLBACK_DIR/$stem-before.lock.json" \
    "$RESET_ROLLBACK_DIR/$stem-pre-mutation.lock.json"
done

while IFS= read -r n; do
  gh issue reopen "$n" --comment 'Product Reset M0 rollback: restoring the exact pre-M0 issue state because the replacement authority did not land or was reverted. Re-triage before implementation.'
done < <(jq -r '.[] | select(.state == "CLOSED") | .number' "$RESET_POST_M0_ISSUES")

while IFS= read -r operation; do
  n=$(jq -r '.number' <<<"$operation")
  while IFS= read -r label; do
    gh issue edit "$n" --remove-label "$label"
  done < <(jq -r '.remove[]' <<<"$operation")
  while IFS= read -r label; do
    gh issue edit "$n" --add-label "$label"
  done < <(jq -r '.add[]' <<<"$operation")
done < <(jq -c '.[]' "$RESET_ROLLBACK_DIR/issue-label-rollback.json")
```

The new labels may remain unused; deleting repository labels is not required for functional
rollback and should be a separate owner decision.

## Restore the old pull request and retire replacement work

```bash
set -e
for n in 126 129 130; do
  gh issue edit "$n" --remove-parent
done
for n in 137 138 139; do
  gh issue edit "$n" --remove-parent
done
gh issue edit 138 --remove-blocked-by 137
gh issue edit 139 --remove-blocked-by 137 --remove-blocked-by 138

gh pr reopen 133 --comment 'Product Reset M0 rollback: reopening because the replacement authority did not land or was reverted. Review threads and checks must be re-evaluated from current head before any merge decision.'

for n in 137 138 139 136; do
  gh issue close "$n" --reason 'not planned' --comment 'Product Reset M0 rollback: this replacement issue is closed because its repository authority did not land or was reverted. See the reopened pre-reset issues.'
done
```

Do not delete the Product Reset issue bodies, branch, comments, or labels; they are rollback audit
evidence.

## Post-rollback verification

```bash
set -euo pipefail
test -n "${RESET_ROLLBACK_DIR:-}"
test -n "${RESET_ORIGINAL_ISSUES:-}"
test -n "${RESET_REPLACEMENT_ISSUES:-}"
test -n "${RESET_PRE_M0_ISSUES:-}"
test -n "${RESET_POST_M0_ISSUES:-}"
test "$(sha256sum "$RESET_PRE_M0_ISSUES" | cut -d ' ' -f 1)" = \
  ba5600b60f829cc1081a0920e57d2883f566c105c47c70df28d72c12745f600e
test "$(sha256sum "$RESET_POST_M0_ISSUES" | cut -d ' ' -f 1)" = \
  a1e888398f18c0731db7545277e381c7637c8c3d11b394703b380acb51e3d47a

gh issue list --state all --limit 200 --json number,state,labels \
  > "$RESET_ROLLBACK_DIR/issues-after.json"
gh pr view 131 --json number,state,mergedAt,headRefName \
  > "$RESET_ROLLBACK_DIR/pr-131-after.json"
gh pr view 133 --json number,state,mergedAt,headRefName \
  > "$RESET_ROLLBACK_DIR/pr-133-after.json"
gh api graphql -f query='query { repository(owner:"ojungo69", name:"free-mem") {
  i126: issue(number:126) { number parent { number } }
  i129: issue(number:129) { number parent { number } }
  i130: issue(number:130) { number parent { number } }
  i136: issue(number:136) { number state subIssues(first:20) { nodes { number state } } }
  i137: issue(number:137) { number state parent { number } blockedBy(first:20) { nodes { number } } }
  i138: issue(number:138) { number state parent { number } blockedBy(first:20) { nodes { number } } }
  i139: issue(number:139) { number state parent { number } blockedBy(first:20) { nodes { number } } }
} }' > "$RESET_ROLLBACK_DIR/relationships-after.json"

jq -S '
  [.[] | {number, state, labels: (.labels | sort)}]
  | sort_by(.number)
' "$RESET_PRE_M0_ISSUES" \
  > "$RESET_ROLLBACK_DIR/issues-expected.normalized.json"
jq -S --argjson original "$RESET_ORIGINAL_ISSUES" '
  [.[]
    | select(.number as $n | ($original | index($n)) != null)
    | {number, state, labels: ([.labels[].name] | sort)}]
  | sort_by(.number)
' "$RESET_ROLLBACK_DIR/issues-after.json" \
  > "$RESET_ROLLBACK_DIR/issues-after.normalized.json"
diff -u \
  "$RESET_ROLLBACK_DIR/issues-expected.normalized.json" \
  "$RESET_ROLLBACK_DIR/issues-after.normalized.json"

mutated=$(jq -cn \
  --argjson original "$RESET_ORIGINAL_ISSUES" \
  --argjson replacements "$RESET_REPLACEMENT_ISSUES" \
  '$original + $replacements | unique')
jq -S --argjson mutated "$mutated" '
  map(select(.number as $n | $mutated | index($n) | not)) | sort_by(.number)
' "$RESET_ROLLBACK_DIR/issues-before.json" \
  > "$RESET_ROLLBACK_DIR/issues-untouched-before.json"
jq -S --argjson mutated "$mutated" '
  map(select(.number as $n | $mutated | index($n) | not)) | sort_by(.number)
' "$RESET_ROLLBACK_DIR/issues-after.json" \
  > "$RESET_ROLLBACK_DIR/issues-untouched-after.json"
diff -u \
  "$RESET_ROLLBACK_DIR/issues-untouched-before.json" \
  "$RESET_ROLLBACK_DIR/issues-untouched-after.json"

jq -S . "$RESET_ROLLBACK_DIR/pr-131-before.json" \
  > "$RESET_ROLLBACK_DIR/pr-131-before.sorted.json"
jq -S . "$RESET_ROLLBACK_DIR/pr-131-after.json" \
  > "$RESET_ROLLBACK_DIR/pr-131-after.sorted.json"
diff -u \
  "$RESET_ROLLBACK_DIR/pr-131-before.sorted.json" \
  "$RESET_ROLLBACK_DIR/pr-131-after.sorted.json"
jq -e '.state == "OPEN" and .mergedAt == null' "$RESET_ROLLBACK_DIR/pr-133-after.json"
jq -e '
  .data.repository
  | (.i126.parent == null and .i129.parent == null and .i130.parent == null)
  and (.i136.state == "CLOSED" and (.i136.subIssues.nodes | length) == 0)
  and all(.i137,.i138,.i139; .state == "CLOSED" and .parent == null and (.blockedBy.nodes | length) == 0)
' "$RESET_ROLLBACK_DIR/relationships-after.json"

for n in 136 137 138 139; do
  gh issue view "$n" --json state,comments \
    | jq -e '.state == "CLOSED" and any(.comments[]; .body | contains("Product Reset M0 rollback"))'
done
```

Expected minimum state:

- the original 69 issues exactly match the committed pre-M0 state and complete label sets;
- PR #133 is open and remains unmerged;
- replacement issues #136-#139 are closed with rollback comments;
- #131 remains unchanged;
- no claim is made that the old continuity findings are fixed or merge-ready.

Record the actual timestamp, command results, and any deviations in a new rollback comment on
\#136. If any issue cannot be restored exactly, stop and document the difference rather than
guessing labels or relationships.
