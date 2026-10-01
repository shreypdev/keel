#!/usr/bin/env bash
# Manual interop check, not part of `cargo test`: the *shipped* TypeScript and Kotlin `remote`
# transports (unmodified) against this crate's real server, over real sockets.
#
#   crates/undra-transport/interop/run.sh            # both, whichever toolchains exist
#   crates/undra-transport/interop/run.sh ts|kotlin
#
# Needs node + npm (TypeScript) and kotlinc + a JDK 11+ (Kotlin; see scripts/env.sh). Everything
# it builds goes into a temporary directory, not the repository. (The Swift transport was run
# the same way once, from a throwaway XCTest; there is no script for it because it has to live
# inside the Swift package.)
set -euo pipefail
cd "$(dirname "$0")/../../.."
WHICH="${1:-all}"
WORK="$(mktemp -d)"
trap '[ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null; rm -rf "$WORK"' EXIT

cargo build -p undra-transport --example serve
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
  cp -R runtimes/ts/@undra/runtime "$WORK/ts"
  (cd "$WORK/ts" && npm ci --no-audit --no-fund --ignore-scripts >/dev/null && npx tsc -p tsconfig.build.json --outDir "$WORK/ts-dist")
  ( node crates/undra-transport/interop/ts.mjs "$BIN" "$WORK/ts-dist" )
fi

if [ "$WHICH" = all ] || [ "$WHICH" = kotlin ]; then
  # shellcheck disable=SC1091
  source scripts/env.sh
  command -v java >/dev/null 2>&1 && java -version >/dev/null 2>&1 || export PATH="/opt/homebrew/opt/openjdk@17/bin:$PATH"
  kotlinc -nowarn -cp "$UNDRA_KOTLINX_COROUTINES" -d "$WORK/kt" \
    $(find runtimes/kotlin/undra-runtime/runtime/src/main -name '*.kt') crates/undra-transport/interop/Interop.kt
  start_server                       # not in $(...): the server must outlive a subshell
  INFO="$(cat "$WORK/server.json")"
  field() { python3 -c "import json,sys; print(json.loads(sys.argv[1])[sys.argv[2]])" "$INFO" "$1"; }
  java -cp "$WORK/kt:$UNDRA_KOTLINX_COROUTINES:$UNDRA_KOTLIN_STDLIB" InteropKt \
    "$(field url)" "$(field schema)" "$(field counter)" "$(field new)" "$(field add)" \
    "$(field sum)" "$(field ask)" "$(field echoPort)" "$(field echoMethod)" "$BIN"
  stop_server
fi
echo "interop OK"
