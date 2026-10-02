#!/usr/bin/env bash
# Runs the two-core test app on Node (ADR-044): builds both cores for the web (`undra build` is
# incremental, so an up-to-date core costs nothing and a stale one, older than the core's sources, is
# never run against newer bindings), then runs main.ts with Node's own TypeScript support (Node 22.18
# or newer) and the source hooks.
#
#   examples/two-cores/node/run.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
[ -x "$UNDRA" ] || (cd "$REPO" && cargo build -p undra-cli)
for ns in a b; do "$UNDRA" build -C "$HERE/../$ns" --platform web; done
cd "$HERE"
# The default Kv of the web is IndexedDB, which Node has not: fake-indexeddb stands in for it, so that each core's
# default store can be written to and the names it is kept under checked.
[ -d node_modules/fake-indexeddb ] || npm ci --no-audit --no-fund
exec node --experimental-transform-types --no-warnings --import ./register.mjs main.ts
