#!/usr/bin/env bash
# Builds the fixture core for wasm32 and runs both wasm test files against it.
#
#   crates/keel-ffi/tests/wasm/run.sh                 # debug build
#   PROFILE=release-wasm crates/keel-ffi/tests/wasm/run.sh   # size-optimised, panic=abort (what ships)
#
# The TypeScript run needs the runtime built: `npm ci && npx tsc -p tsconfig.build.json` in
# runtimes/ts/@keel/runtime (done here if node_modules exists but dist does not).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
PROFILE="${PROFILE:-dev}"

if [ "$PROFILE" = "dev" ]; then
  cargo build --manifest-path "$HERE/../fixture/Cargo.toml" --target wasm32-unknown-unknown
else
  cargo build --manifest-path "$HERE/../fixture/Cargo.toml" --target wasm32-unknown-unknown --profile "$PROFILE"
fi

DIR="$PROFILE"; [ "$DIR" = "dev" ] && DIR="debug"
export KEEL_FFI_FIXTURE_WASM="${KEEL_FFI_FIXTURE_WASM:-$HERE/../fixture/target/wasm32-unknown-unknown/$DIR/keel_core.wasm}"

node --test "$HERE/raw.test.mjs"

TS="$REPO/runtimes/ts/@keel/runtime"
if [ -z "${KEEL_TS_DIST:-}" ] && [ ! -f "$TS/dist/index.js" ] && [ -d "$TS/node_modules" ]; then
  (cd "$TS" && npx tsc -p tsconfig.build.json)
fi
if [ -n "${KEEL_TS_DIST:-}" ] || [ -f "$TS/dist/index.js" ]; then
  node --test "$HERE/ts-runtime.test.mjs"
else
  echo "note: TypeScript runtime not built; skipping ts-runtime.test.mjs" >&2
fi
