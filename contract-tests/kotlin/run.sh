#!/usr/bin/env bash
# The Kotlin column of the contract tests: runs S01..S17 of contract-tests/scenarios.md on the JVM over
# JNI against the real libkeel_core of the playground core, then checks all seventeen passed.
#
#   contract-tests/kotlin/run.sh
#
# What it does, each step only when something changed:
#   1. builds the host core with the keel CLI (`keel build -C examples/playground --platform host`) when
#      build/host/libkeel_core.* is missing or older than the core's sources
#   2. compiles the Kotlin runtime's main sources (runtimes/kotlin/keel-runtime/scripts/test-local.sh main)
#   3. compiles the generated bindings (examples/playground/generated/kotlin) together with the runner (src/)
#   4. runs the runner on the JVM with -Djava.library.path pointing at the core, and pipes its output
#      through contract-tests/check.sh kotlin
#
# Output goes under contract-tests/kotlin/build (git-ignored); KEEL_BUILD_DIR moves it. Needs the toolchain
# scripts/env.sh sets up (JDK 17, kotlinc, KEEL_KOTLINX_COROUTINES, KEEL_KOTLIN_STDLIB) and cargo.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"            # contract-tests/kotlin
REPO="$(cd "$HERE/../.." && pwd)"
PLAYGROUND="$REPO/examples/playground"
OUT="${KEEL_BUILD_DIR:-$HERE/build}"

if ! command -v kotlinc >/dev/null 2>&1 || ! command -v java >/dev/null 2>&1 || [ -z "${KEEL_KOTLINX_COROUTINES:-}" ]; then
  # shellcheck disable=SC1091
  source "$REPO/scripts/env.sh"
fi
for tool in kotlinc java cargo; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: $tool is not on PATH (source scripts/env.sh)" >&2; exit 2; }
done
: "${KEEL_KOTLINX_COROUTINES:?set KEEL_KOTLINX_COROUTINES to kotlinx-coroutines-core-jvm-1.6.4.jar (scripts/env.sh does)}"
: "${KEEL_KOTLIN_STDLIB:?set KEEL_KOTLIN_STDLIB to kotlin-stdlib.jar (scripts/env.sh does)}"

# --- 1. the core: the host library `keel build` writes --------------------------------------------------
LIB_DIR="$PLAYGROUND/build/host"
LIB=""
for candidate in "$LIB_DIR/libkeel_core.dylib" "$LIB_DIR/libkeel_core.so"; do
  if [ -f "$candidate" ]; then LIB="$candidate"; fi
done
stale=0
if [ -z "$LIB" ]; then
  stale=1
elif [ -n "$(find "$PLAYGROUND/core" "$PLAYGROUND/keel.toml" "$REPO/crates" \( -name '*.rs' -o -name 'Cargo.toml' -o -name keel.toml \) -newer "$LIB" -print -quit)" ]; then
  stale=1
fi
if [ "$stale" = 1 ]; then
  KEEL="$REPO/target/debug/keel"
  if [ ! -x "$KEEL" ]; then
    echo "==> building the keel CLI"
    cargo build --manifest-path "$REPO/Cargo.toml" -p keel-cli
  fi
  echo "==> keel build -C examples/playground --platform host"
  "$KEEL" build -C "$PLAYGROUND" --platform host
fi

# --- 2. the Kotlin runtime (main sources only) -----------------------------------------------------------
KEEL_BUILD_DIR="$OUT/runtime" "$REPO/runtimes/kotlin/keel-runtime/scripts/test-local.sh" main

# --- 3. the generated bindings and the runner ------------------------------------------------------------
GENERATED="$PLAYGROUND/generated/kotlin/src/main/kotlin"
CLASSES="$OUT/classes"
STAMP="$OUT/classes.stamp"
if [ ! -f "$STAMP" ] || [ "$OUT/runtime/main.stamp" -nt "$STAMP" ] \
   || [ -n "$(find "$HERE/src" "$GENERATED" -type f -newer "$STAMP" -print -quit)" ]; then
  echo "==> compiling the bindings and the runner"
  rm -rf "$CLASSES" "$STAMP"
  mkdir -p "$CLASSES"
  kotlinc -cp "$OUT/runtime/main:$KEEL_KOTLINX_COROUTINES" -jvm-target 11 -d "$CLASSES" "$GENERATED" "$HERE/src"
  touch "$STAMP"
fi

# --- 4. run, then check the verdicts ------------------------------------------------------------------------
echo "==> running S01..S17 against ${LIB#"$REPO"/}"
mkdir -p "$OUT"
status=0
java -Xmx1g -Djava.library.path="$LIB_DIR" \
  -cp "$OUT/runtime/main:$CLASSES:$KEEL_KOTLIN_STDLIB:$KEEL_KOTLINX_COROUTINES" \
  dev.keel.contract.MainKt 2>&1 | tee "$OUT/run.log" || status=$?
"$REPO/contract-tests/check.sh" kotlin "$OUT/run.log" || status=1
exit "$status"
