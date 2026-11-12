#!/usr/bin/env bash
set -euo pipefail

die() { printf '%s\n' "$*" >&2; exit 1; }
[[ "$#" -eq 2 ]] || die 'usage: trial-smoke.sh <archive> <expected version line>'
archive="$1"
expected="$2"
[[ -f "$archive" && ! -L "$archive" ]] || die 'archive must be a regular file'
file="${archive##*/}"
case "$file" in
  *.tar.gz) name="${file%.tar.gz}" ;;
  *.zip) name="${file%.zip}" ;;
  *) die 'invalid archive extension' ;;
esac
case "$name" in
  oboete-*-x86_64-unknown-linux-gnu) target=x86_64-unknown-linux-gnu ;;
  oboete-*-aarch64-apple-darwin) target=aarch64-apple-darwin ;;
  oboete-*-x86_64-pc-windows-msvc) target=x86_64-pc-windows-msvc ;;
  *) die 'invalid archive target' ;;
esac
version="${name#oboete-}"
version="${version%-"$target"}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)*-trial\.[1-9][0-9]*$ ]] || die 'invalid trial version'
exe=oboete
if [[ "$target" == x86_64-pc-windows-msvc ]]; then
  [[ "$file" == *.zip ]] || die 'Windows needs a zip archive'
  exe=oboete.exe
else
  [[ "$file" == *.tar.gz ]] || die 'Unix needs a tar.gz archive'
fi
temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT
python=python3
case "${OSTYPE:-}" in msys*|cygwin*) python=python ;; esac
# Check names and types before extraction: an archive must not write outside this folder.
"$python" - "$archive" "$temporary" "$name" "$exe" <<'PY'
import stat
import sys
import tarfile
import zipfile

archive, destination, folder, binary = sys.argv[1:]
expected = {folder + "/"} | {folder + "/" + f for f in (binary, "LICENSE", "NOTICE", "README.md")}
if archive.endswith(".zip"):
    with zipfile.ZipFile(archive) as packed:
        entries = packed.infolist()
        names = [e.filename for e in entries]
        regular = all(e.is_dir() if e.filename == folder + "/" else
                      not e.is_dir() and stat.S_IFMT(e.external_attr >> 16) in (0, stat.S_IFREG)
                      for e in entries)
        if len(names) != 5 or set(names) != expected or not regular:
            raise SystemExit("archive must contain only the named folder and its four regular files")
        packed.extractall(destination)
else:
    with tarfile.open(archive, "r:gz") as packed:
        entries = packed.getmembers()
        names = [e.name.rstrip("/") + "/" if e.isdir() else e.name for e in entries]
        regular = all(e.isdir() if n == folder + "/" else e.isfile()
                      for e, n in zip(entries, names))
        if len(names) != 5 or set(names) != expected or not regular:
            raise SystemExit("archive must contain only the named folder and its four regular files")
        packed.extractall(destination)
PY
binary="$temporary/$name/$exe"
"$binary" --version > "$temporary/version"
printf '%s\n' "$expected" > "$temporary/expected"
# Compare files so an extra blank line is a failure too.
cmp -s "$temporary/expected" "$temporary/version" || die "unexpected version (expected: $expected)"
trial_home="$(mktemp -d "$temporary/home.XXXXXX")"
"$binary" --home "$trial_home" doctor
printf 'PASS: %s\n' "$file"
