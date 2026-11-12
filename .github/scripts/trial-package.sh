#!/usr/bin/env bash
set -euo pipefail

die() { printf '%s\n' "$*" >&2; exit 1; }
[[ "$#" -eq 5 ]] || die 'usage: trial-package.sh <binary> <version> <target> <commit> <output-directory>'
binary="$1"
version="$2"
target="$3"
commit="$4"
output="$5"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)*-trial\.[1-9][0-9]*$ ]] || die 'invalid trial version'
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || die 'invalid commit'
exe=oboete
format=gztar
extension=tar.gz
case "$target" in
  x86_64-unknown-linux-gnu) floor='needs glibc 2.28 or later' ;;
  aarch64-apple-darwin) floor='needs macOS 11 or later (Apple silicon)' ;;
  x86_64-pc-windows-msvc) floor='x86_64'; exe=oboete.exe; format=zip; extension=zip ;;
  *) die 'invalid target' ;;
esac
[[ -f "$binary" && ! -L "$binary" ]] || die 'binary must be a regular file'
repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
name="oboete-$version-$target"
mkdir -p -- "$output"
output="$(cd -- "$output" && pwd)"
archive="$output/$name.$extension"
[[ ! -e "$archive" ]] || die "archive already exists: $archive"
staging="$(mktemp -d)"
trap 'rm -rf -- "$staging"' EXIT
mkdir -- "$staging/$name"
cp -- "$binary" "$staging/$name/$exe"
chmod 755 "$staging/$name/$exe"
cp -- "$repo/LICENSE" "$repo/NOTICE" "$staging/$name/"
cat > "$staging/$name/README.md" <<EOF
# oboete $version

This is a trial build of software that is not finished.
https://github.com/ojungo69/oboete

Commit: $commit
Target: $target, $floor

Put $exe where it will stay, for example ~/.local/bin. The agents' hooks call
it by its path. Add that directory to PATH, then run:

    oboete setup <agent>
    oboete doctor

Download SHA256SUMS beside the archive and verify the download:

    sha256sum -c --ignore-missing SHA256SUMS
    gh attestation verify $name.$extension --repo ojungo69/oboete
EOF
# Python's archive formats work on all three runners, including Git Bash on Windows.
python=python3
case "${OSTYPE:-}" in msys*|cygwin*) python=python ;; esac
"$python" - "$output/$name" "$format" "$staging" "$name" <<'PY'
import shutil
import sys

print(shutil.make_archive(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]))
PY
