#!/usr/bin/env bash
# Work-view spike: saves the spike viewer's answers for one home with the viewer's token, which is
# read here and never printed. Usage: VIEWER_OUT=<the viewer's output file> bash get.sh <out dir> <path>...
# (a path is e.g. "/api/my-work?range=week"); VIEWER_USER is the isolated user (oboete-dogfood).
set -u
out=$1; shift
mkdir -p "$out"
T=$(sudo -n -u "${VIEWER_USER:-oboete-dogfood}" -H cat "${VIEWER_OUT:?the viewer output file}" | grep -oE '#t=[A-Za-z0-9_-]+' | head -1 | cut -c4-)
[ -n "$T" ] || { echo "no token"; exit 1; }
for p in "$@"; do
  name=$(printf '%s' "$p" | tr -c 'A-Za-z0-9' '_' | sed -E 's/^_+|_+$//g')
  # The header on stdin (-H @-): an argument would show the token to every user's ps.
  code=$(printf 'X-Oboete-Token: %s\n' "$T" | curl -s -o "$out/$name.json" -w '%{http_code} %{size_download} %{time_total}' \
    -H @- "http://127.0.0.1:17381$p")
  echo "$p -> $code"
done
