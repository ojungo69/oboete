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
# What it replaces must be a plain folder and a plain file: a link would take the moves below to
# another place.
if [ -L "$here/upstream" ] || [ -L "$here/UPSTREAM" ] || [ -d "$here/UPSTREAM" ]; then
  echo "fetch.sh: upstream/ or UPSTREAM is a link, or UPSTREAM is a folder" >&2
  exit 1
fi

# One reserved folder beside upstream/ holds the new tree, its UPSTREAM line and, while they move
# in, the old tree. Each move is a rename on one file system. However the script ends (an error,
# Ctrl-C, a kill), an unfinished swap puts the old tree back, so the tree on disk is always the one
# UPSTREAM names, and a tree that cannot be put back is kept in the reserved folder.
swap=$(mktemp -d "$here/.upstream.XXXXXX")
cleanup() {
  if [ -e "$swap/old" ] && { [ -e "$swap/new" ] || [ -e "$swap/UPSTREAM" ]; }; then
    rm -rf "${here:?}/upstream"
    if ! mv "$swap/old" "$here/upstream"; then
      echo "fetch.sh: the old tree is kept in $swap/old" >&2
      return 0
    fi
  fi
  rm -rf "$swap"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP
mkdir "$swap/new"
git -C "$src" archive --format=tar "$commit" "${paths[@]}" | tar -x -C "$swap/new"
chmod 755 "$swap/new"
printf 'claude-mem %s, commit %s\n' "$tag" "$commit" > "$swap/UPSTREAM"
if [ -e "$here/upstream" ]; then
  mv "$here/upstream" "$swap/old"
fi
mv "$swap/new" "$here/upstream"
mv "$swap/UPSTREAM" "$here/UPSTREAM"
echo "upstream/ is claude-mem $tag ($commit): $(find "$here/upstream" -type f | wc -l) files"
