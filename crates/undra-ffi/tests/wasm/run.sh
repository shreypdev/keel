#!/usr/bin/env bash
# Builds the fixture core for wasm32 and runs both wasm test files against it.
#
#   crates/undra-ffi/tests/wasm/run.sh                 # debug build
#   PROFILE=release-wasm crates/undra-ffi/tests/wasm/run.sh   # size-optimised, panic=abort (what ships)
#
# The TypeScript run builds the runtime from source into a scratch directory every time (`npm ci`
# first when node_modules is missing); see the end of this file. It drives the core in both wasm modes:
# `wasm-main` on the test's own thread and `wasm-worker` on a real `worker_threads` Worker (Clock, Rng and
# Log answered inside the worker, async host ports through the main thread, snapshot and restore).
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
export UNDRA_FFI_FIXTURE_WASM="${UNDRA_FFI_FIXTURE_WASM:-$HERE/../fixture/target/wasm32-unknown-unknown/$DIR/undra_core.wasm}"

node --test "$HERE/raw.test.mjs"

# The TypeScript leg always runs against a TypeScript runtime built from the sources of this checkout,
# into a scratch directory: it can never pass (or be skipped) because of a stale or missing dist/.
#   UNDRA_TS_DIST=/path/to/dist/index.js   use a runtime you built yourself instead
#   UNDRA_SKIP_TS=1                        skip this leg on purpose (it says so)
TS="$REPO/runtimes/ts/@undra/runtime"
if [ "${UNDRA_SKIP_TS:-0}" = "1" ]; then
  echo "note: UNDRA_SKIP_TS=1, skipping ts-runtime.test.mjs" >&2
  exit 0
fi
if [ -z "${UNDRA_TS_DIST:-}" ]; then
  TSOUT="$(mktemp -d)"
  trap 'rm -rf "$TSOUT"' EXIT
  [ -d "$TS/node_modules" ] || (cd "$TS" && npm ci)
  (cd "$TS" && npx tsc -p tsconfig.build.json --outDir "$TSOUT")
  export UNDRA_TS_DIST="$TSOUT/index.js"
fi
node --test "$HERE/ts-runtime.test.mjs"
