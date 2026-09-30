#!/usr/bin/env bash
# Manual interop check, not part of `cargo test`: the *shipped* TypeScript and Kotlin `remote`
# transports (unmodified) against this crate's real server, over real sockets.
#
#   crates/keel-transport/interop/run.sh            # both, whichever toolchains exist
#   crates/keel-transport/interop/run.sh ts|kotlin
#
# Needs node + npm (TypeScript) and kotlinc + a JDK 11+ (Kotlin; see scripts/env.sh). Everything
# it builds goes into a temporary directory, not the repository. (The Swift transport was run
# the same way once, from a throwaway XCTest; there is no script for it because it has to live
# inside the Swift package.)
set -euo pipefail
cd "$(dirname "$0")/../../.."
WHICH="${1:-all}"
WORK="$(mktemp -d)"
trap 'kill "${SERVER_PID:-0}" 2>/dev/null || true; rm -rf "$WORK"' EXIT

cargo build -p keel-transport --example serve
BIN="$PWD/target/debug/examples/serve"

# The server runs until its stdin closes; a fifo held open keeps it up.
start_server() {
  mkfifo "$WORK/stdin"
  "$BIN" < "$WORK/stdin" > "$WORK/server.json" 2> "$WORK/server.log" &
  SERVER_PID=$!
  exec 9> "$WORK/stdin"
  for _ in $(seq 50); do [ -s "$WORK/server.json" ] && break; sleep 0.1; done
}
stop_server() { exec 9>&-; wait "$SERVER_PID" 2>/dev/null || true; rm -f "$WORK/stdin"; }

if [ "$WHICH" = all ] || [ "$WHICH" = ts ]; then
  cp -R runtimes/ts/@keel/runtime "$WORK/ts"
  (cd "$WORK/ts" && npm ci --no-audit --no-fund --ignore-scripts >/dev/null && npx tsc -p tsconfig.build.json --outDir "$WORK/ts-dist")
  ( node crates/keel-transport/interop/ts.mjs "$BIN" "$WORK/ts-dist" )
fi

if [ "$WHICH" = all ] || [ "$WHICH" = kotlin ]; then
  # shellcheck disable=SC1091
  source scripts/env.sh
  command -v java >/dev/null 2>&1 && java -version >/dev/null 2>&1 || export PATH="/opt/homebrew/opt/openjdk@17/bin:$PATH"
  kotlinc -nowarn -cp "$KEEL_KOTLINX_COROUTINES" -d "$WORK/kt" \
    $(find runtimes/kotlin/keel-runtime/runtime/src/main -name '*.kt') crates/keel-transport/interop/Interop.kt
  start_server                       # not in $(...): the server must outlive a subshell
  INFO="$(cat "$WORK/server.json")"
  field() { python3 -c "import json,sys; print(json.loads(sys.argv[1])[sys.argv[2]])" "$INFO" "$1"; }
  java -cp "$WORK/kt:$KEEL_KOTLINX_COROUTINES:$KEEL_KOTLIN_STDLIB" InteropKt \
    "$(field url)" "$(field schema)" "$(field counter)" "$(field new)" "$(field add)" \
    "$(field sum)" "$(field ask)" "$(field echoPort)" "$(field echoMethod)"
  stop_server
fi
echo "interop OK"
