#!/usr/bin/env bash
# §13.6 external supervisor — sidecar subprocess を隔離実行する。
#   - hard deadline (DEADLINE 秒、既定 20)
#   - 専用 session/process group で起動し、期限超過/終了後に group 全体へ TERM → KILL
#   - stdout は上限 (CAP bytes、既定 1MiB) まで取り込み、pipe close。stderr も同じ上限で bound
#   - wait/reap + 残存 descendant 検査。**setsid で group を抜けた子孫も検出する**ため、
#     検査は PGID だけでなく「起動時に存在しなかった、自分の子孫」を pid の親子鎖で辿る
# 使い方: supervisor.sh <out-file> <cmd...>   (stdin は /dev/null)
#
# 検出できない範囲（明示）: 起動前から存在した pid を再利用したプロセス、
# 別ユーザー権限で fork されたプロセスは追跡対象外。
set -u
OUT="${1:?out-file}"; shift
DEADLINE="${DEADLINE:-20}"
CAP="${CAP:-1048576}"
SUP_PGID="$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ')"

# 起動前スナップショット（この後に現れた PID だけを子孫候補にする）
BEFORE="$(ps -eo pid= 2>/dev/null | tr -d ' ' | sort -u)"

setsid "$@" < /dev/null > >(head -c "$CAP" > "$OUT") 2> >(head -c "$CAP" > "$OUT.err") &
PID=$!

# PGID 取得は execve/setsid と競合する。取れるまで少しだけ待ち、
# 取れない/自分と同じ場合は group kill を使わず PID 単体で扱う（自爆防止）。
PGID=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  PGID="$(ps -o pgid= -p "$PID" 2>/dev/null | tr -d ' ')"
  [ -n "$PGID" ] && [ "$PGID" != "$SUP_PGID" ] && break
  kill -0 "$PID" 2>/dev/null || break     # もう終わっている
  sleep 0.1
done
if [ -z "$PGID" ] || [ "$PGID" = "$SUP_PGID" ]; then
  PGID=""                                  # group kill は行わない
fi

kill_target() {  # $1 = シグナル
  if [ -n "$PGID" ]; then kill "-$1" -- "-$PGID" 2>/dev/null; fi
  kill "-$1" "$PID" 2>/dev/null
}

# 子孫集合は **kill する前に** 採取する。親を殺すと孤児は init に再親付けされ
# 祖先鎖が切れるため、事後の tree 走査では setsid 脱走子を取りこぼす。
DESC_FILE="$(mktemp)"
snapshot_descendants() {
  ps -eo pid=,ppid=,pgid= 2>/dev/null | awk -v root="$PID" -v g="${PGID:-0}" '
    { pid[$1]=1; par[$1]=$2; grp[$1]=$3 }
    END {
      for (p in pid) {
        if (p == root) { print p; continue }
        if (g != 0 && grp[p] == g) { print p; continue }
        q = p; depth = 0
        while (q in par && q != 1 && depth < 64) {
          if (q == root) { print p; break }
          q = par[q]; depth++
        }
      }
    }' >> "$DESC_FILE"
}

end=$(( $(date +%s) + DEADLINE ))
tick=0
while kill -0 "$PID" 2>/dev/null && [ "$(date +%s)" -lt "$end" ]; do
  tick=$((tick+1))
  [ $((tick % 3)) -eq 1 ] && snapshot_descendants     # 約 0.6 秒ごと
  sleep 0.2
done
snapshot_descendants

TIMED_OUT=0
if kill -0 "$PID" 2>/dev/null; then
  TIMED_OUT=1
  kill_target TERM
  for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
  kill_target KILL
fi
wait "$PID" 2>/dev/null; RC=$?

# stdout/stderr を取り込む head が終わるまで待つ（process substitution は
# 孫プロセスなので wait できない。fd を掴んだままの子孫がいると EOF が来ないため上限付きで待つ）
for _ in $(seq 1 25); do
  pgrep -P $$ >/dev/null 2>&1 || break
  sleep 0.2
done

# 記録済み子孫のうち、起動前から存在した PID（pid 再利用の誤検出）を除いて生存判定する。
sleep 0.3
SURV=""
for p in $(sort -un "$DESC_FILE"); do
  [ "$p" = "$PID" ] && continue
  printf '%s\n' "$BEFORE" | grep -qx "$p" && continue
  kill -0 "$p" 2>/dev/null && SURV="$SURV$p "
done
rm -f "$DESC_FILE"

if [ -n "${SURV// /}" ]; then
  for s in $SURV; do kill -KILL "$s" 2>/dev/null; done   # fail-closed
  echo "SUPERVISOR: rc=$RC timedOut=$TIMED_OUT survivors=[${SURV% }] (killed)"
  exit 70
fi
echo "SUPERVISOR: rc=$RC timedOut=$TIMED_OUT survivors=[]"
[ "$TIMED_OUT" -eq 1 ] && exit 71
exit "$RC"
