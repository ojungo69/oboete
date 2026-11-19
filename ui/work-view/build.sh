#!/usr/bin/env bash
# Builds the work-view oboete serves into assets/work-view/ (decision 114): claude-mem's tree
# (upstream/) with oboete's patches (patches/, in order), its packages installed with install
# scripts off, and THIRD-PARTY-NOTICES.md for each package and font the bundle holds.
# The bundle is committed; CI builds it again and fails when the two differ.
# Usage: ui/work-view/build.sh
set -euo pipefail
# The scripts' own globbing, whatever a caller's exported SHELLOPTS, BASHOPTS or GLOBIGNORE say: the
# guard and the patch list below read their folders through globs.
set +f
shopt -u failglob nullglob nocaseglob
unset GLOBIGNORE

# The folder this script is in, with links resolved, and the checkout's own assets/ beside it: a
# link there would make the moves below act on another folder, so it stops instead.
here=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
case $here in
  */ui/work-view) ;;
  *) echo "build.sh: $here is not ui/work-view" >&2; exit 1 ;;
esac
assets="${here%/ui/work-view}/assets"
if [ -L "$assets" ] || [ ! -d "$assets" ] || [ -L "$assets/work-view" ]; then
  echo "build.sh: $assets or its work-view is a link or missing" >&2
  exit 1
fi

# One build or fetch at a time in a checkout: a second one would move its tree into the first
# one's. The lock is a folder, made or refused in one step; a run that was killed leaves it behind.
lock="$here/.lock"
if ! mkdir "$lock" 2>/dev/null; then
  echo "build.sh: another build or fetch holds $lock; remove it if none is running" >&2
  exit 1
fi

work=
swap=
# However the script ends (an error, Ctrl-C, a kill other than SIGKILL), an unfinished swap puts the
# old bundle back, and a bundle that cannot be put back is kept in its reserved folder rather than
# removed. Each step runs whatever failed before it (set +e), and no signal stops the clean-up
# halfway. A run whose clean-up left the lock, the old bundle, the reserved folder or the scratch
# tree fails, whatever came before.
# A SIGKILL runs no handler: the README says how to recover.
cleanup() {
  local status=$? failed=
  trap '' INT TERM HUP
  set +e
  if [ -n "$swap" ]; then
    if [ -e "$swap/old" ] && [ ! -e "$assets/work-view" ] && ! mv "$swap/old" "$assets/work-view"; then
      echo "build.sh: the old bundle is kept in $swap/old" >&2
      failed=1
    elif ! rm -rf "$swap"; then
      echo "build.sh: $swap is left" >&2
      failed=1
    fi
  fi
  if [ -n "$work" ] && ! rm -rf "$work"; then
    echo "build.sh: $work is left" >&2
    failed=1
  fi
  if ! rmdir "$lock"; then
    echo "build.sh: $lock is left; remove it if no build or fetch is running" >&2
    failed=1
  fi
  [ "$status" -ne 0 ] || [ -z "$failed" ] || status=1
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP
# A reserved folder of fetch.sh's that holds an UPSTREAM line is a fetch that did not finish: its
# UPSTREAM is what says so, as a folder left only because its removal failed holds none. The trees
# in place may not be the pair UPSTREAM names, so nothing runs until it is recovered (README.md, "A
# run that was killed"). mktemp makes the folder its owner's alone: one this user cannot look in,
# as when the build runs as another user than the fetch did, stops the run too.
if [ ! -r "$here" ]; then
  echo "build.sh: $(id -un) cannot list $here, so a fetch that did not finish there would go unseen; see ui/work-view/README.md, \"A run that was killed\"" >&2
  exit 1
fi
for left in "$here"/.upstream.*; do
  [ -d "$left" ] || continue
  if [ ! -r "$left" ] || [ ! -x "$left" ]; then
    echo "build.sh: $(id -un) cannot look in $left, which may be a fetch that did not finish; see ui/work-view/README.md, \"A run that was killed\"" >&2
    exit 1
  fi
  if [ -e "$left/UPSTREAM" ]; then
    echo "build.sh: $left is a fetch that did not finish; see ui/work-view/README.md, \"A run that was killed\"" >&2
    exit 1
  fi
done
work=$(mktemp -d)
# An absolute path: Git ignores a relative entry of GIT_CEILING_DIRECTORIES, as a relative TMPDIR
# would give. CDPATH is cleared, as a cd it resolved would print the folder too, and mktemp's path
# stays for the clean-up until this one is made.
abs=$(CDPATH='' cd -- "$work" && pwd -P)
work=$abs
cp -a "$here/upstream/." "$work/"
# GIT_CEILING_DIRECTORIES: git apply patches the copy as plain files even when the temporary
# folder lies inside a checkout. There it would read the patch's paths from the checkout's top,
# skip each one, and succeed with nothing applied; a file it skips stops the build all the same.
# oboete's patches carry its token, its transport and its name: a build without one is not the
# work-view oboete serves, so an empty patches/ stops it. The list is made once, the checkout's
# own path taken as it is, whatever characters it holds.
shopt -s nullglob
patches=("$here"/patches/*.patch)
shopt -u nullglob
if [ "${#patches[@]}" -eq 0 ]; then
  echo "build.sh: $here/patches holds no patch" >&2
  exit 1
fi
for patch in "${patches[@]}"; do
  # LC_ALL=C: Git words its messages in the caller's language, and the skip is read in English.
  # --whitespace=nowarn: a warning prints the line it is about by itself, which a line of the patch
  # that opens with "Skipped patch" would make read as a skip. The patch applies as it is either way.
  if ! out=$(cd "$work" && LC_ALL=C GIT_CEILING_DIRECTORIES=$(dirname "$work") git apply --verbose --whitespace=nowarn "$patch" 2>&1); then
    printf '%s\n' "$out" >&2
    exit 1
  fi
  case $'\n'$out in
    *$'\nSkipped patch'*) printf 'build.sh: %s skipped a file:\n%s\n' "$patch" "$out" >&2; exit 1 ;;
  esac
done
# The files outside src/ui/my-work import packages from claude-mem's root node_modules, which its
# own root install provides. Here they resolve to the work-view's own packages.
ln -s src/ui/my-work/node_modules "$work/node_modules"
app="$work/src/ui/my-work"
# The tools come from the lockfile alone: dev dependencies installed whatever npm's settings say,
# and run from node_modules/.bin rather than fetched by npx. --sourcemap hidden: maps for the
# notices, with no reference to them in the files served.
# Before the build, claude-mem's own tests run on the patched tree, oboete's beside them (#1085):
# package.json's test command, run by node with each test held to a minute and the run to 15
# minutes, so a test that never ends fails by its name rather than holding the job. A test command
# package.json no longer names would leave tests out: the build stops instead.
tests='node --test src/live/*.test.ts src/setup/*.test.ts src/store/*.test.ts'
(cd "$app" && npm ci --ignore-scripts --include=dev --no-audit --no-fund \
  && if [ "$(node -p "require('./package.json').scripts.test")" != "$tests" ]; then
    echo "build.sh: package.json's test command is no longer $tests" >&2
    exit 1
  fi \
  && timeout 900 node --test --test-timeout=60000 src/live/*.test.ts src/setup/*.test.ts src/store/*.test.ts \
  && ./node_modules/.bin/tsc -b && ./node_modules/.bin/vite build --sourcemap hidden)
node "$here/notices.mjs" "$work" "$here/licences" > "$app/dist/THIRD-PARTY-NOTICES.md"
find "$app/dist" -name '*.map' -delete

# One reserved folder beside assets/work-view/ holds the new bundle and, while it moves in, the
# old one: each move is a rename, the old bundle is put back if the new one cannot move in, and
# nothing outside the reserved folder is removed.
swap=$(mktemp -d "$assets/.work-view.XXXXXX")
cp -a "$app/dist" "$swap/new"
chmod 755 "$swap/new"
if [ -e "$assets/work-view" ]; then
  mv "$assets/work-view" "$swap/old"
fi
mv "$swap/new" "$assets/work-view"
echo "assets/work-view: $(find "$assets/work-view" -type f | wc -l) files, $(du -sb "$assets/work-view" | cut -f1) bytes"
