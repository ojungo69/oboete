#!/usr/bin/env bash
set -u

LOG_PATH_ENV="${CODEMEM_PLUGIN_LOG_PATH:-${CODEMEM_PLUGIN_LOG:-}}"
case "${LOG_PATH_ENV}" in
  ""|"0"|"false"|"off") LOG_PATH="$HOME/.codemem/plugin.log" ;;
  "1"|"true"|"yes") LOG_PATH="$HOME/.codemem/plugin.log" ;;
  *) LOG_PATH="${LOG_PATH_ENV}" ;;
esac

log_line() {
  mkdir -p "$(dirname "${LOG_PATH}")" >/dev/null 2>&1 || true
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"${LOG_PATH}" 2>/dev/null || true
}

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

if command -v node >/dev/null 2>&1 && [ -r "${SCRIPT_DIR}/hook-runtime.mjs" ]; then
  exec node "${SCRIPT_DIR}/hook-runtime.mjs" claude-hook-ingest
fi
log_line "codemem claude-hook-ingest skipped: bundled runtime unavailable"
exit 0
