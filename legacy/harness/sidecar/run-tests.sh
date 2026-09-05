#!/usr/bin/env bash
# supervisor の §13.6 要件を stub subprocess で実証する self-check (資格情報不要・不活性)。
set -u
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUP="$DIR/supervisor.sh"; STUB="$DIR/stub-misbehaver.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fails=0
chk(){ if [ "$1" = ok ]; then echo "PASS: $2"; else echo "FAIL: $2"; fails=$((fails+1)); fi }

# 1) normal: rc=0, JSON 出力, 残存なし
out="$T/normal.out"; DEADLINE=10 "$SUP" "$out" "$STUB" normal > "$T/normal.sup"; rc=$?
grep -q '"ok":true' "$out" && [ $rc -eq 0 ] && grep -q 'survivors=\[\]' "$T/normal.sup" && chk ok "normal run" || chk no "normal run"

# 2) hang: deadline で刈られ rc=71, 残存なし
out="$T/hang.out"; DEADLINE=3 "$SUP" "$out" "$STUB" hang > "$T/hang.sup"; rc=$?
[ $rc -eq 71 ] && grep -q 'timedOut=1' "$T/hang.sup" && grep -q 'survivors=\[\]' "$T/hang.sup" && chk ok "hang killed at deadline" || chk no "hang killed at deadline"

# 3) SIGTERM 無視でも KILL 昇格で死ぬ
out="$T/it.out"; DEADLINE=3 "$SUP" "$out" "$STUB" ignore-term > "$T/it.sup"; rc=$?
[ $rc -eq 71 ] && grep -q 'survivors=\[\]' "$T/it.sup" && chk ok "ignore-term killed via KILL escalation" || chk no "ignore-term killed via KILL escalation"

# 4) TERM 無視の孫プロセス込みで process-group kill が全滅させる
out="$T/od.out"; DEADLINE=3 "$SUP" "$out" "$STUB" orphan-descendants > "$T/od.sup"; rc=$?
grep -q 'survivors=\[\]' "$T/od.sup" && chk ok "descendants killed by group kill" || chk no "descendants killed by group kill"

# 5) 切断 JSON は parser がハングせずエラーとして表面化する
#    （空ファイルでも catch に落ちるため、出力が非空であることを先に確認する）
out="$T/tj.out"; DEADLINE=10 "$SUP" "$out" "$STUB" truncated-json > "$T/tj.sup"; rc=$?
if [ -s "$out" ] && node -e 'try{JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));process.exit(1)}catch{process.exit(0)}' "$out"; then
  chk ok "truncated JSON surfaces as parse error (non-empty capture)"
else
  chk no "truncated JSON surfaces as parse error (non-empty capture)"
fi

# 6) setsid で group を抜けた子孫も検出・駆除される（PGID 走査だけでは漏れる形）
out="$T/se.out"; DEADLINE=3 "$SUP" "$out" "$STUB" setsid-escape > "$T/se.sup"; rc=$?
sleep 0.5
escaped_alive="$(ps -eo args= | grep -c '[s]leep 600' || true)"
if [ "$escaped_alive" -eq 0 ] && grep -q 'survivors=\[[0-9]' "$T/se.sup"; then
  chk ok "setsid escapee detected as survivor and killed (fail-closed)"
else
  chk no "setsid escapee detected and killed (alive=$escaped_alive, $(grep -o 'survivors=\[[^]]*\]' "$T/se.sup"))"
  ps -eo pid=,args= | awk '/[s]leep 600/{print $1}' | xargs -r kill -KILL 2>/dev/null
fi

# 7) 出力上限 (CAP) に達したら打ち切られ、supervisor は終了する
out="$T/fl.out"; DEADLINE=10 CAP=4096 "$SUP" "$out" "$STUB" flood-stdout > "$T/fl.sup"; rc=$?
sz=$(stat -c%s "$out" 2>/dev/null || echo 0)
if [ "$sz" -le 4096 ] && [ "$sz" -gt 0 ] && grep -q 'survivors=\[\]' "$T/fl.sup"; then
  chk ok "stdout capped at CAP bytes and process reaped (got ${sz}B)"
else
  chk no "stdout capped at CAP bytes and process reaped (got ${sz}B)"
fi

echo "---"
if [ $fails -eq 0 ]; then echo "ALL PASS"; else echo "$fails FAILED"; exit 1; fi
