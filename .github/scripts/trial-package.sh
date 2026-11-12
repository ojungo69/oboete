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
# What prints a file's SHA-256 digest where the target runs.
case "$target" in
  x86_64-unknown-linux-gnu) floor='needs glibc 2.28 or later'; digest='sha256sum' ;;
  aarch64-apple-darwin) floor='needs macOS 11 or later (Apple silicon)'; digest='shasum -a 256' ;;
  x86_64-pc-windows-msvc)
    floor='needs Windows 10 or later (x86_64)'
    digest='Get-FileHash -Algorithm SHA256'
    exe=oboete.exe; format=zip; extension=zip ;;
  *) die 'invalid target' ;;
esac
# The repository the build is of: the run's, or oboete's own when run by hand.
repository="${GITHUB_REPOSITORY:-ojungo69/oboete}"
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || die 'invalid repository'
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
https://github.com/$repository

Commit: $commit
Target: $target, $floor

Put $exe where it will stay, for example ~/.local/bin. The agents' hooks call
it by its path. Add that directory to PATH, then run:

    oboete setup <agent>
    oboete doctor

To check the download, print its SHA-256 digest: it is the one on the archive's
line in SHA256SUMS, which is beside the archive on its release's page.

    $digest $name.$extension

Its build provenance:

    gh attestation verify $name.$extension --repo $repository
EOF
# Python's archive formats work on all three runners, including Git Bash on Windows.
python=python3
case "${OSTYPE:-}" in msys*|cygwin*) python=python ;; esac
"$python" - "$output/$name" "$format" "$staging" "$name" <<'PY'
import shutil
import sys

print(shutil.make_archive(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]))
PY
