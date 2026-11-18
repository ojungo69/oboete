#!/usr/bin/env bash
# Work-view spike, pass item 7: the median time of /api/work-view against /api/my-work plus
# /api/my-goals, on the viewer running now. The token is read here and never printed.
# Usage: VIEWER_OUT=<the viewer's output file> timing.sh <runs>
set -u
n=$1
T=$(sudo -n -u "${VIEWER_USER:-oboete-dogfood}" -H cat "${VIEWER_OUT:?the viewer output file}" | grep -oE '#t=[A-Za-z0-9_-]+' | head -1 | cut -c4-)
[ -n "$T" ] || { echo "no token"; exit 1; }
for p in "/api/work-view" "/api/my-work?range=week" "/api/my-goals"; do
  times=()
  for i in $(seq 1 "$n"); do
    times+=("$(curl -s -o /dev/null -w '%{time_total} %{size_download}' -H "X-Oboete-Token: $T" "http://127.0.0.1:17381$p")")
  done
  printf '%s\n' "${times[@]}" | python3 -I -c '
import sys, statistics
rows = [l.split() for l in sys.stdin if l.strip()]
t = [float(r[0]) * 1000 for r in rows]
print(f"{sys.argv[1]}: first {t[0]:.1f} ms, median of the rest {statistics.median(t[1:]):.1f} ms, max {max(t[1:]):.1f} ms, {rows[-1][1]} bytes")
' "$p"
done
