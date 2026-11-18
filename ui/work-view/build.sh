#!/usr/bin/env bash
# Builds the work-view oboete serves into assets/work-view/ (decision 114): claude-mem's tree
# (upstream/) with oboete's patches (patches/, in order), its packages installed with install
# scripts off, and THIRD-PARTY-NOTICES.md for each package and font the bundle holds.
# The bundle is committed; CI builds it again and fails when the two differ.
# Usage: ui/work-view/build.sh
set -euo pipefail

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

work=$(mktemp -d)
swap=
trap 'rm -rf "$work" ${swap:+"$swap"}' EXIT
cp -a "$here/upstream/." "$work/"
for patch in "$here"/patches/*.patch; do
  (cd "$work" && git apply "$patch")
done
# The files outside src/ui/my-work import packages from claude-mem's root node_modules, which its
# own root install provides. Here they resolve to the work-view's own packages.
ln -s src/ui/my-work/node_modules "$work/node_modules"
app="$work/src/ui/my-work"
# The tools come from the lockfile alone: dev dependencies installed whatever npm's settings say,
# and run from node_modules/.bin rather than fetched by npx. --sourcemap hidden: maps for the
# notices, with no reference to them in the files served.
(cd "$app" && npm ci --ignore-scripts --include=dev --no-audit --no-fund \
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
if ! mv "$swap/new" "$assets/work-view"; then
  if [ -e "$swap/old" ] && ! mv "$swap/old" "$assets/work-view"; then
    echo "build.sh: the old bundle is kept in $swap/old" >&2
    swap=
  fi
  exit 1
fi
echo "assets/work-view: $(find "$assets/work-view" -type f | wc -l) files, $(du -sb "$assets/work-view" | cut -f1) bytes"
