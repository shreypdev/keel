#!/usr/bin/env bash
# Runs the two-core test app on the JVM (ADR-044): builds both cores for this machine
# (libplayground_a, libplayground_b), compiles the runtime, both generated packages and Main.kt, and runs it
# with both libraries on java.library.path. Needs the toolchain scripts/env.sh sets up (kotlinc, JDK 17,
# UNDRA_KOTLINX_COROUTINES, UNDRA_KOTLIN_STDLIB).
#
#   examples/two-cores/jvm/run.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
OUT="${UNDRA_BUILD_DIR:-$HERE/build}"
if ! command -v kotlinc >/dev/null 2>&1 || [ -z "${UNDRA_KOTLINX_COROUTINES:-}" ]; then
  # shellcheck disable=SC1091
  source "$REPO/scripts/env.sh"
fi
[ -x "$UNDRA" ] || (cd "$REPO" && cargo build -p undra-cli)
for ns in a b; do "$UNDRA" build -C "$HERE/../$ns" --platform host; done
UNDRA_BUILD_DIR="$OUT/runtime" "$REPO/runtimes/kotlin/undra-runtime/scripts/test-local.sh" main
mkdir -p "$OUT/classes"
kotlinc -cp "$OUT/runtime/main:$UNDRA_KOTLINX_COROUTINES" -jvm-target 11 -d "$OUT/classes" \
  "$HERE/../a/generated/kotlin/src/main/kotlin" "$HERE/../b/generated/kotlin/src/main/kotlin" "$HERE/Main.kt"
# The default stores live in a data directory of their own for this run (per core namespace under it), so what the
# app finds there is what it wrote.
DATA="$(mktemp -d)"
trap 'rm -rf "$DATA"' EXIT
java -Dundra.data.dir="$DATA" -Djava.library.path="$HERE/../a/build/host:$HERE/../b/build/host" \
  -cp "$OUT/runtime/main:$OUT/classes:$UNDRA_KOTLIN_STDLIB:$UNDRA_KOTLINX_COROUTINES" dev.undra.twocores.jvm.MainKt
