#!/usr/bin/env bash
# The Kotlin column of the contract tests: runs S01..S19 of contract-tests/scenarios.md on the JVM over
# JNI against the real libundra_core of the playground core, then checks all nineteen passed.
#
#   contract-tests/kotlin/run.sh
#
# What it does:
#   1. builds the host core twice with the undra CLI (`undra build -C examples/playground --platform host`,
#      incremental): first build B of the migration steps (`UNDRA_PLAYGROUND_V2=1`, scenarios.md "Two
#      builds"), whose library is copied aside to $OUT/core-b, then build A, the default, copied to
#      $OUT/core-a (each JVM loads its own copy, so a later `undra build` cannot swap it under the run)
#   2. compiles the Kotlin runtime's main sources (runtimes/kotlin/undra-runtime/scripts/test-local.sh main)
#   3. compiles the generated bindings (examples/playground/generated/kotlin) together with the runner (src/)
#   4. runs the runner on the JVM with -Djava.library.path pointing at build A; S14 and S15 hand what build A
#      persisted over in $OUT/migration
#   5. runs it again in a second JVM over build B (UNDRA_CONTRACT_PHASE=B), which checks S14 steps 8-9 and
#      S15 steps 12-14 and prints only `SCENARIO S14|S15 FAIL` lines (plus `MIGRATION ... ok`)
#   6. pipes the output of both through contract-tests/check.sh kotlin (the last line of an id counts)
#
# Output goes under contract-tests/kotlin/build (git-ignored); UNDRA_BUILD_DIR moves it. Needs the toolchain
# scripts/env.sh sets up (JDK 17, kotlinc, UNDRA_KOTLINX_COROUTINES, UNDRA_KOTLIN_STDLIB) and cargo. kotlinc is
# the one on PATH (put another first to compile with it; use a separate UNDRA_BUILD_DIR per compiler).
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

# --- 1. the cores: build B (kept aside), then build A ------------------------------------------------------
UNDRA="$REPO/target/debug/undra"
if [ ! -x "$UNDRA" ]; then
  echo "==> building the undra CLI"
  cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
fi
LIB_DIR="$PLAYGROUND/build/host"
CORE_A="$OUT/core-a"
CORE_B="$OUT/core-b"

# Copies the library `undra build` just wrote to $1 (under its usual name, which is what System.loadLibrary finds).
stage_core() {
  local lib="" candidate
  for candidate in "$LIB_DIR/libundra_core.dylib" "$LIB_DIR/libundra_core.so"; do
    if [ -f "$candidate" ]; then lib="$candidate"; fi
  done
  if [ -z "$lib" ]; then echo "error: undra build wrote no libundra_core under $LIB_DIR" >&2; exit 1; fi
  rm -rf "$1"
  mkdir -p "$1"
  cp "$lib" "$1/"
}

echo "==> UNDRA_PLAYGROUND_V2=1 undra build -C examples/playground --platform host (build B)"
UNDRA_PLAYGROUND_V2=1 "$UNDRA" build -C "$PLAYGROUND" --platform host
stage_core "$CORE_B"
echo "==> undra build -C examples/playground --platform host (build A)"
"$UNDRA" build -C "$PLAYGROUND" --platform host
stage_core "$CORE_A"
if cmp -s "$CORE_A"/libundra_core.* "$CORE_B"/libundra_core.*; then
  echo "error: build B's library is identical to build A's; UNDRA_PLAYGROUND_V2=1 did not reach the core" >&2
  exit 1
fi

# --- 2. the Kotlin runtime (main sources only) -----------------------------------------------------------
UNDRA_BUILD_DIR="$OUT/runtime" "$REPO/runtimes/kotlin/undra-runtime/scripts/test-local.sh" main

# --- 3. the generated bindings and the runner ------------------------------------------------------------
GENERATED="$PLAYGROUND/generated/kotlin/src/main/kotlin"
CLASSES="$OUT/classes"
STAMP="$OUT/classes.stamp"
if [ ! -f "$STAMP" ] || [ "$OUT/runtime/main.stamp" -nt "$STAMP" ] \
   || [ -n "$(find "$HERE/src" "$GENERATED" -type f -newer "$STAMP" -print -quit)" ]; then
  echo "==> compiling the bindings and the runner"
  rm -rf "$CLASSES" "$STAMP"
  mkdir -p "$CLASSES"
  kotlinc -cp "$OUT/runtime/main:$UNDRA_KOTLINX_COROUTINES" -jvm-target 11 -d "$CLASSES" "$GENERATED" "$HERE/src"
  touch "$STAMP"
fi

# --- 4. run build A, then 5. build B's steps in a second JVM, then 6. check the verdicts ------------------
HANDOVER="$OUT/migration"
rm -rf "$HANDOVER"
export UNDRA_CONTRACT_HANDOVER="$HANDOVER"
CP="$OUT/runtime/main:$CLASSES:$UNDRA_KOTLIN_STDLIB:$UNDRA_KOTLINX_COROUTINES"
echo "==> running S01..S19 against build A (${CORE_A#"$REPO"/})"
mkdir -p "$OUT"
status=0
java -Xmx1g -Djava.library.path="$CORE_A" -cp "$CP" dev.undra.contract.MainKt 2>&1 | tee "$OUT/run.log" || status=$?
echo "==> running the build-B steps of S14 and S15 against build B (${CORE_B#"$REPO"/})"
UNDRA_CONTRACT_PHASE=B java -Xmx1g -Djava.library.path="$CORE_B" -cp "$CP" dev.undra.contract.MainKt 2>&1 | tee -a "$OUT/run.log" || status=$?
"$REPO/contract-tests/check.sh" kotlin "$OUT/run.log" || status=1
exit "$status"
