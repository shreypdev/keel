#!/usr/bin/env bash
# Runs the two-core test app on Node (ADR-044): builds both cores for the web when they are missing,
# then runs main.ts with Node's own TypeScript support (Node 22.18 or newer) and the source hooks.
#
#   examples/two-cores/node/run.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
for ns in a b; do
  if [ ! -f "$HERE/../$ns/build/web/playground_$ns.wasm" ]; then
    [ -x "$UNDRA" ] || (cd "$REPO" && cargo build -p undra-cli)
    "$UNDRA" build -C "$HERE/../$ns" --platform web
  fi
done
cd "$HERE"
exec node --experimental-transform-types --no-warnings --import ./register.mjs main.ts
