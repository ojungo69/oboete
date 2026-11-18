#!/usr/bin/env bash
# Builds the work-view oboete serves into assets/work-view/ (decision 114): claude-mem's tree
# (upstream/) with oboete's patches (patches/, in order), its packages installed with install
# scripts off, and THIRD-PARTY-NOTICES.md for each package and font the bundle holds.
# The bundle is committed; CI builds it again and fails when the two differ.
# Usage: ui/work-view/build.sh
set -euo pipefail

# The folder this script is in, with links resolved: it replaces assets/work-view/ beside it.
here=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
case $here in
  */ui/work-view) ;;
  *) echo "build.sh: $here is not ui/work-view" >&2; exit 1 ;;
esac
assets=$(cd "$here/../../assets" && pwd -P)

work=$(mktemp -d)
staged=
trap 'rm -rf "$work" ${staged:+"$staged"}' EXIT
cp -a "$here/upstream/." "$work/"
for patch in "$here"/patches/*.patch; do
  (cd "$work" && git apply "$patch")
done
# The files outside src/ui/my-work import packages from claude-mem's root node_modules, which its
# own root install provides. Here they resolve to the work-view's own packages.
ln -s src/ui/my-work/node_modules "$work/node_modules"
app="$work/src/ui/my-work"
# --sourcemap hidden: maps for the notices, with no reference to them in the files served.
(cd "$app" && npm ci --ignore-scripts --no-audit --no-fund && npx tsc -b && npx vite build --sourcemap hidden)
node "$here/notices.mjs" "$work" "$here/licences" > "$app/dist/THIRD-PARTY-NOTICES.md"
find "$app/dist" -name '*.map' -delete

# Staged beside assets/work-view/ and moved in by rename, the old bundle kept until the new is in.
staged=$(mktemp -d "$assets/.work-view.XXXXXX")
cp -a "$app/dist/." "$staged/"
chmod 755 "$staged"
old="$staged.old"
if [ -e "$assets/work-view" ]; then
  mv "$assets/work-view" "$old"
fi
if ! mv "$staged" "$assets/work-view"; then
  if [ -e "$old" ]; then mv "$old" "$assets/work-view"; fi
  exit 1
fi
staged=
rm -rf "$old"
echo "assets/work-view: $(find "$assets/work-view" -type f | wc -l) files, $(du -sb "$assets/work-view" | cut -f1) bytes"
