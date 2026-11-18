#!/usr/bin/env bash
# Work-view spike, pass item 7: the median time of /api/work-view against /api/my-work plus
# /api/my-goals, on the viewer running now. The token is read here and never printed.
# Usage: VIEWER_OUT=<the viewer's output file> bash timing.sh <runs>. A failed request stops it.
# At least 2 runs: the first is reported apart, and the median and the maximum are of the rest.
set -u
n=${1:-}
case $n in
  '' | *[!0-9]*) echo "runs: a whole number, at least 2"; exit 1 ;;
esac
[ "$n" -ge 2 ] || { echo "runs: at least 2"; exit 1; }
T=$(sudo -n -u "${VIEWER_USER:-oboete-dogfood}" -H cat "${VIEWER_OUT:?the viewer output file}" | grep -oE '#t=[A-Za-z0-9_-]+' | head -1 | cut -c4-)
[ -n "$T" ] || { echo "no token"; exit 1; }
for p in "/api/work-view" "/api/my-work?range=week" "/api/my-goals"; do
  times=()
  for _ in $(seq 1 "$n"); do
    # The header on stdin (-H @-): an argument would show the token to every user's ps.
    got=$(printf 'X-Oboete-Token: %s\n' "$T" | curl -s -o /dev/null -w '%{http_code} %{time_total} %{size_download}' \
      -H @- "http://127.0.0.1:17381$p") || { echo "$p: the request failed"; exit 1; }
    [ "${got%% *}" = 200 ] || { echo "$p: HTTP ${got%% *}"; exit 1; }
    times+=("${got#* }")
  done
  printf '%s\n' "${times[@]}" | python3 -I -c '
import sys, statistics
rows = [l.split() for l in sys.stdin if l.strip()]
t = [float(r[0]) * 1000 for r in rows]
print(f"{sys.argv[1]}: first {t[0]:.1f} ms, median of the rest {statistics.median(t[1:]):.1f} ms, max {max(t[1:]):.1f} ms, {rows[-1][1]} bytes")
' "$p"
done
