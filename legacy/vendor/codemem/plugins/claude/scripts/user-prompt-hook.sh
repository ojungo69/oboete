#!/usr/bin/env bash
set -u

print_continue() {
  printf '%s\n' '{"continue":true}'
}

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
if command -v node >/dev/null 2>&1 && [ -r "${SCRIPT_DIR}/hook-runtime.mjs" ]; then
  exec node "${SCRIPT_DIR}/hook-runtime.mjs" claude-hook-inject
fi
payload="$(cat)"

if [ -z "${payload}" ]; then
  print_continue
  exit 0
fi

if [ -x "${SCRIPT_DIR}/inject-context-hook.sh" ]; then
  if ! printf '%s' "${payload}" | "${SCRIPT_DIR}/inject-context-hook.sh"; then
    print_continue
  fi
else
  if ! printf '%s' "${payload}" | bash "${SCRIPT_DIR}/inject-context-hook.sh"; then
    print_continue
  fi
fi
