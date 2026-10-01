#!/usr/bin/env bash
# Runs JniE2E.kt against the fixture core over the real JNI shim.
#
#   crates/undra-ffi/tests/jni/run.sh
#
# Needs the toolchain scripts/env.sh sets up (kotlinc, JDK 17, UNDRA_KOTLINX_COROUTINES,
# UNDRA_KOTLIN_STDLIB) and builds what it uses: the Kotlin runtime (test-local.sh main) and the fixture
# cdylib (libundra_core, feature jni).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
if ! command -v kotlinc >/dev/null 2>&1 || ! command -v java >/dev/null 2>&1; then
  # shellcheck disable=SC1091
  source "$REPO/scripts/env.sh"
fi
OUT="${UNDRA_BUILD_DIR:-$REPO/runtimes/kotlin/undra-runtime/build/local}"

cargo build --manifest-path "$HERE/../fixture/Cargo.toml"
"$REPO/runtimes/kotlin/undra-runtime/scripts/test-local.sh" main
mkdir -p "$OUT/jni-e2e"
kotlinc -cp "$OUT/main:$UNDRA_KOTLINX_COROUTINES" -d "$OUT/jni-e2e" "$HERE/JniE2E.kt"
java -Xmx512m -Djava.library.path="$HERE/../fixture/target/debug" \
  -cp "$OUT/main:$OUT/jni-e2e:$UNDRA_KOTLIN_STDLIB:$UNDRA_KOTLINX_COROUTINES" JniE2EKt
