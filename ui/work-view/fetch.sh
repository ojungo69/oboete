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

# One build or fetch at a time in a checkout: a second one would move its tree into the first
# one's. The lock is a folder, made or refused in one step; a run that was killed leaves it behind.
lock="$here/.lock"
if ! mkdir "$lock" 2>/dev/null; then
  echo "fetch.sh: another build or fetch holds $lock; remove it if none is running" >&2
  exit 1
fi

# One reserved folder beside upstream/ holds the new tree, its UPSTREAM line and, while they move
# in, the old tree. Each move is a rename on one file system. The swap is done when the UPSTREAM
# line has moved in. However the script ends before that (an error, Ctrl-C, a kill other than
# SIGKILL), the tree this run put in goes and the old one comes back, so the tree on disk is always
# the one UPSTREAM names; a tree that cannot be put back is kept in the reserved folder. Each step
# runs whatever failed before it (set +e). A SIGKILL runs no handler: the README says how to recover.
swap=
moving=
cleanup() {
  set +e
  if [ -n "$moving" ] && [ -e "$swap/UPSTREAM" ]; then
    if [ ! -e "$swap/new" ]; then rm -rf "${here:?}/upstream"; fi
    # A tree left in its place (its removal failed) would take the old one in as a folder of its own.
    if [ -e "$swap/old" ] && { [ -e "$here/upstream" ] || ! mv "$swap/old" "$here/upstream"; }; then
      echo "fetch.sh: the old tree is kept in $swap/old" >&2
      swap=
    fi
  fi
  rm -rf ${swap:+"$swap"}
  rmdir "$lock" || echo "fetch.sh: $lock is left; remove it if no build or fetch is running" >&2
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP
swap=$(mktemp -d "$here/.upstream.XXXXXX")
mkdir "$swap/new"
git -C "$src" archive --format=tar "$commit" "${paths[@]}" | tar -x -C "$swap/new"
chmod 755 "$swap/new"
printf 'claude-mem %s, commit %s\n' "$tag" "$commit" > "$swap/UPSTREAM"
moving=1
if [ -e "$here/upstream" ]; then
  mv "$here/upstream" "$swap/old"
fi
mv "$swap/new" "$here/upstream"
mv "$swap/UPSTREAM" "$here/UPSTREAM"
echo "upstream/ is claude-mem $tag ($commit): $(find "$here/upstream" -type f | wc -l) files"
