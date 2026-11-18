#!/usr/bin/env bash
# Replaces ui/work-view/upstream/ with claude-mem's work-view sources at a tag, unchanged, and
# records the tag and its commit in ui/work-view/UPSTREAM (docs/spike/work-view.md, "Next").
# Usage: ui/work-view/fetch.sh <a claude-mem checkout> <tag>
set -euo pipefail
src=${1:?a claude-mem checkout}
tag=${2:?a tag, e.g. v15.19.0}

# The folder this script is in, with links resolved: it replaces its own upstream/ and nothing else.
here=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
case $here in
  */ui/work-view) ;;
  *) echo "fetch.sh: $here is not ui/work-view" >&2; exit 1 ;;
esac

# The work-view and every file outside src/ui/my-work it imports, with claude-mem's src/ layout
# (its relative imports climb to src/). The root package.json is there for its "type": "module".
paths=(
  package.json LICENSE NOTICE
  src/ui/my-work src/ui/i18n src/ui/ui-message.ts
  src/ui/install-local-api-token.ts src/ui/local-api-token-client.ts
  src/ui/viewer/utils/sync-presentation.ts src/ui/viewer/utils/viewer-activity.ts
  src/shared/conversation-continuation.ts src/shared/setup-funnel.ts
  src/shared/setup-install.ts src/shared/focus-change-version.ts
  src/services/work-view/nav-words.mjs src/services/work-view/nav-words.d.mts
  src/services/work-view/my-focus.d.mts src/services/work-view/my-rules.d.mts
)

commit=$(git -C "$src" rev-parse --verify "$tag^{commit}")
# The new tree and its UPSTREAM line are staged beside upstream/, so that each move below is a
# rename on one file system. The old tree is kept until both are in place, and put back if either
# move fails: the tree on disk is always the one UPSTREAM names.
staged=$(mktemp -d "$here/.upstream.XXXXXX")
old="$staged.old"
record="$staged.UPSTREAM"
trap 'rm -rf "$staged" "$record"' EXIT
git -C "$src" archive --format=tar "$commit" "${paths[@]}" | tar -x -C "$staged"
chmod 755 "$staged"
printf 'claude-mem %s, commit %s\n' "$tag" "$commit" > "$record"
if [ -e "$here/upstream" ]; then
  mv "$here/upstream" "$old"
fi
if ! mv "$staged" "$here/upstream" || ! mv "$record" "$here/UPSTREAM"; then
  rm -rf "${here:?}/upstream"
  if [ -e "$old" ]; then mv "$old" "$here/upstream"; fi
  exit 1
fi
trap - EXIT
rm -rf "$old"
echo "upstream/ is claude-mem $tag ($commit): $(find "$here/upstream" -type f | wc -l) files"
