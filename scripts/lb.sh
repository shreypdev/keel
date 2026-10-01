#!/usr/bin/env bash
# Local build wrapper for the Cowork VM: every shell call is killed after ~180 s, so long cargo
# jobs are run in slices. Re-run the same command until it prints DONE. Cargo caches finished
# crates in target/, so each slice makes progress.
#   scripts/lb.sh cargo test -p undra-wire
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
SLICE="${UNDRA_SLICE_SECONDS:-160}"
timeout --foreground -s INT "$SLICE" "$@"
code=$?
if [ "$code" -eq 124 ]; then
  echo "SLICE-TIMEOUT: re-run the same command to continue (progress is cached)."
  exit 124
fi
if [ "$code" -eq 0 ]; then echo "DONE"; else echo "FAILED exit=$code"; fi
exit "$code"
