#!/usr/bin/env bash
# Claude Code / Codex hook から呼ばれ、stdin の payload を 1 行 JSON で追記する。
# 引数: $1 = hook event 名。出力先: $CAPTURE_FILE（rig.sh が設定）。
set -u
EVENT="${1:-unknown}"
OUT="${CAPTURE_FILE:?CAPTURE_FILE unset — run under rig.sh}"
# 試験用 knob: INJECT_MARKER = hook stdout 注入試験（SessionStart/UserPromptSubmit の
# stdout は context に入る仕様の実証）。HOOK_SLEEP = hook timeout 挙動試験（記録後に sleep）。
if [ -n "${INJECT_MARKER:-}" ] && { [ "$EVENT" = SessionStart ] || [ "$EVENT" = UserPromptSubmit ]; }; then
  printf '%s\n' "$INJECT_MARKER"
fi
node -e '
const chunks=[];process.stdin.on("data",c=>chunks.push(c)).on("end",()=>{
  const raw=Buffer.concat(chunks).toString("utf8");
  let payload; try{payload=JSON.parse(raw)}catch{payload={unparsed:raw}}
  const line=JSON.stringify({event:process.argv[1],at:new Date().toISOString(),payload});
  require("fs").appendFileSync(process.argv[2],line+"\n");
});' "$EVENT" "$OUT"
REC_RC=$?
# 記録失敗は「hook が発火しなかった」と区別できなければ matrix の unknown cell と
# 見分けがつかなくなる。失敗痕跡を別ファイルに必ず残す（CLI 側は 0 で通す）。
if [ "$REC_RC" -ne 0 ]; then
  printf '%s\trecorder-failed rc=%s event=%s\n' "$(date -Iseconds)" "$REC_RC" "$EVENT" >> "$OUT.errors" 2>/dev/null || true
fi
if [ -n "${HOOK_SLEEP:-}" ]; then sleep "$HOOK_SLEEP"; fi
exit 0
