#!/usr/bin/env bash
# The Kotlin column of the contract tests: runs S01..S18 of contract-tests/scenarios.md on the JVM over
# JNI against the real libundra_core of the playground core, then checks all eighteen passed.
#
#   contract-tests/kotlin/run.sh
#
# What it does, each step only when something changed:
#   1. builds the host core with the undra CLI (`undra build -C examples/playground --platform host`) when
#      build/host/libundra_core.* is missing or older than the core's sources
#   2. compiles the Kotlin runtime's main sources (runtimes/kotlin/undra-runtime/scripts/test-local.sh main)
#   3. compiles the generated bindings (examples/playground/generated/kotlin) together with the runner (src/)
#   4. runs the runner on the JVM with -Djava.library.path pointing at the core, and pipes its output
#      through contract-tests/check.sh kotlin
#
# Output goes under contract-tests/kotlin/build (git-ignored); UNDRA_BUILD_DIR moves it. Needs the toolchain
# scripts/env.sh sets up (JDK 17, kotlinc, UNDRA_KOTLINX_COROUTINES, UNDRA_KOTLIN_STDLIB) and cargo.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"            # contract-tests/kotlin
REPO="$(cd "$HERE/../.." && pwd)"
PLAYGROUND="$REPO/examples/playground"
OUT="${UNDRA_BUILD_DIR:-$HERE/build}"

if ! command -v kotlinc >/dev/null 2>&1 || ! command -v java >/dev/null 2>&1 || [ -z "${UNDRA_KOTLINX_COROUTINES:-}" ]; then
  # shellcheck disable=SC1091
  source "$REPO/scripts/env.sh"
fi
for tool in kotlinc java cargo; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: $tool is not on PATH (source scripts/env.sh)" >&2; exit 2; }
done
: "${UNDRA_KOTLINX_COROUTINES:?set UNDRA_KOTLINX_COROUTINES to kotlinx-coroutines-core-jvm-1.6.4.jar (scripts/env.sh does)}"
: "${UNDRA_KOTLIN_STDLIB:?set UNDRA_KOTLIN_STDLIB to kotlin-stdlib.jar (scripts/env.sh does)}"

# --- 1. the core: the host library `undra build` writes --------------------------------------------------
LIB_DIR="$PLAYGROUND/build/host"
LIB=""
for candidate in "$LIB_DIR/libundra_core.dylib" "$LIB_DIR/libundra_core.so"; do
  if [ -f "$candidate" ]; then LIB="$candidate"; fi
done
stale=0
if [ -z "$LIB" ]; then
  stale=1
elif [ -n "$(find "$PLAYGROUND/core" "$PLAYGROUND/undra.toml" "$REPO/crates" \( -name '*.rs' -o -name 'Cargo.toml' -o -name undra.toml \) -newer "$LIB" -print -quit)" ]; then
  stale=1
fi
if [ "$stale" = 1 ]; then
  UNDRA="$REPO/target/debug/undra"
  if [ ! -x "$UNDRA" ]; then
    echo "==> building the undra CLI"
    cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
  fi
  echo "==> undra build -C examples/playground --platform host"
  "$UNDRA" build -C "$PLAYGROUND" --platform host
fi

# --- 2. the Kotlin runtime (main sources only) -----------------------------------------------------------
UNDRA_BUILD_DIR="$OUT/runtime" "$REPO/runtimes/kotlin/undra-runtime/scripts/test-local.sh" main

# --- 2b. the testing kit (dev.undra.testkit), compiled against those runtime classes ----------------------------
KIT_SRC="$REPO/runtimes/kotlin/undra-runtime/testkit/src/main/kotlin"
KIT="$OUT/kit"
if [ ! -f "$OUT/kit.stamp" ] || [ "$OUT/runtime/main.stamp" -nt "$OUT/kit.stamp" ] \
   || [ -n "$(find "$KIT_SRC" -type f -newer "$OUT/kit.stamp" -print -quit)" ]; then
  echo "==> compiling the testing kit"
  rm -rf "$KIT" "$OUT/kit.stamp"
  mkdir -p "$KIT"
  kotlinc -cp "$OUT/runtime/main:$UNDRA_KOTLINX_COROUTINES" -jvm-target 11 -opt-in=dev.undra.runtime.UndraEmbeddingApi -d "$KIT" "$KIT_SRC"
  touch "$OUT/kit.stamp"
fi

# --- 3. the generated bindings and the runner ------------------------------------------------------------
GENERATED="$PLAYGROUND/generated/kotlin/src/main/kotlin"
CLASSES="$OUT/classes"
STAMP="$OUT/classes.stamp"
if [ ! -f "$STAMP" ] || [ "$OUT/runtime/main.stamp" -nt "$STAMP" ] \
   || [ "$OUT/kit.stamp" -nt "$STAMP" ] || [ -n "$(find "$HERE/src" "$GENERATED" -type f -newer "$STAMP" -print -quit)" ]; then
  echo "==> compiling the bindings and the runner"
  rm -rf "$CLASSES" "$STAMP"
  mkdir -p "$CLASSES"
  kotlinc -cp "$OUT/runtime/main:$KIT:$UNDRA_KOTLINX_COROUTINES" -jvm-target 11 -d "$CLASSES" "$GENERATED" "$HERE/src"
  touch "$STAMP"
fi

# --- 4. run, then check the verdicts ------------------------------------------------------------------------
echo "==> running S01..S18 against ${LIB#"$REPO"/}"
mkdir -p "$OUT"
status=0
java -Xmx1g -Djava.library.path="$LIB_DIR" \
  -cp "$OUT/runtime/main:$CLASSES:$UNDRA_KOTLIN_STDLIB:$UNDRA_KOTLINX_COROUTINES" \
  dev.undra.contract.MainKt 2>&1 | tee "$OUT/run.log" || status=$?
"$REPO/contract-tests/check.sh" kotlin "$OUT/run.log" || status=1

# --- 5. the testing kit against the same core (not part of the grid) -------------------------------------------
echo "==> running the testing kit against ${LIB#"$REPO"/}"
java -Xmx1g -Djava.library.path="$LIB_DIR" \
  -cp "$OUT/runtime/main:$KIT:$CLASSES:$UNDRA_KOTLIN_STDLIB:$UNDRA_KOTLINX_COROUTINES" \
  dev.undra.contract.TestKitMainKt 2>&1 | tee "$OUT/testkit.log" || status=$?
exit "$status"
