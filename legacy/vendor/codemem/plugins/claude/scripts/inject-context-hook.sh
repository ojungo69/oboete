#!/usr/bin/env bash
set -u

print_continue() {
  printf '%s\n' '{"continue":true}'
}

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
if command -v node >/dev/null 2>&1 && [ -r "${SCRIPT_DIR}/hook-runtime.mjs" ]; then
  exec node "${SCRIPT_DIR}/hook-runtime.mjs" claude-hook-inject
fi
print_continue
