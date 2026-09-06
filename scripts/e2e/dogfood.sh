#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DST=${OBOETE_DOGFOOD_DIR:-/home/oboete-dogfood/oboete}

case "${1:-}" in
  push)
    sudo -u oboete-dogfood -H mkdir -p "$DST/scripts"
    sudo rm -rf "$DST/scripts/e2e"
    sudo tar -C "$ROOT" -cf - scripts/e2e | sudo -u oboete-dogfood -H tar -C "$DST" -xf -
    sudo chown -R oboete-dogfood:oboete-dogfood "$DST/scripts/e2e"
    ;;
  run)
    shift
    args=$(printf '%q ' "$@")
    dst=$(printf '%q' "$DST")
    sudo -u oboete-dogfood -H bash -lc "export PATH=\"\$HOME/.local/bin:\$HOME/.npm-global/bin:\$PATH\"; cd $dst && node scripts/e2e/probe-contracts.mjs $args"
    ;;
  pull)
    dest=${2:-"$ROOT/.tmp/oboete-probes"}
    mkdir -p "$dest" "$ROOT/docs/research" "$ROOT/test"
    if sudo test -d "$DST/test/contracts"; then
      sudo tar -C "$DST" -cf - test/contracts | tar -C "$ROOT" -xf -
    fi
    latest=$(sudo -u oboete-dogfood -H bash -lc 'ls -1d "$HOME/.cache/oboete-probes"/*/ 2>/dev/null | sort | tail -1' || true)
    if [ -n "${latest:-}" ]; then
      sudo cat "${latest}report.json" > "$dest/report.json"
      runId=$(basename "${latest%/}")
      doc="$ROOT/docs/research/oboete-contracts-probes.md"
      if sudo test -f "${latest}report.md"; then
        if [ ! -f "$doc" ] || ! grep -q "run ${runId}" "$doc"; then
          sudo cat "${latest}report.md" >> "$doc"
        fi
      fi
    fi
    ;;
  *)
    echo "usage: $0 push | run ARGS | pull [report-dest]" >&2
    exit 2
    ;;
esac
