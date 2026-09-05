#!/usr/bin/env bash
# supervisor テスト用 stub subprocess。sidecar の悪性挙動を不活性に模倣する。
# モード: normal | hang | ignore-term | orphan-descendants | truncated-json
set -u
case "${1:-normal}" in
  normal)
    echo '{"ok":true,"mode":"normal"}' ;;
  hang)
    echo '{"partial":true}'; sleep 600 ;;
  ignore-term)
    trap '' TERM
    echo '{"partial":true,"mode":"ignore-term"}'
    sleep 600 ;;
  orphan-descendants)
    ( trap '' TERM; sleep 600 ) &
    ( trap '' TERM; sleep 600 ) &
    echo '{"spawned":2}'
    sleep 600 ;;
  setsid-escape)
    # process group を抜けて逃げる子孫（daemonize する sidecar の現実的な形）
    setsid sleep 600 < /dev/null > /dev/null 2>&1 &
    echo '{"escaped":1}'
    sleep 600 ;;
  flood-stdout)
    yes "$(head -c 200 /dev/zero | tr '\0' 'x')" ;;   # CAP 到達 → pipe close 経路の実証
  truncated-json)
    printf '{"result":"trunc' ;;   # 不正/切断 JSON 耐性テスト用
  *) echo "unknown mode" >&2; exit 2 ;;
esac
