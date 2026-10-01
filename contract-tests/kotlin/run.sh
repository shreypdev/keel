#!/usr/bin/env bash
# The Kotlin column of the contract tests: runs S01..S19 and S26 of contract-tests/scenarios.md on the JVM
# over JNI against the real libplayground_core of the playground core (and, for S26, libplayground_a and
# libplayground_b: the same core under two more namespaces, examples/two-cores), then checks all twenty passed.
#
#   contract-tests/kotlin/run.sh
#
# What it does, each step only when something changed:
#   1. builds the host cores with the undra CLI (`undra build -C <project> --platform host`) when
#      build/host/lib<namespace>.* is missing or older than the core's sources
#   2. compiles the Kotlin runtime's main sources (runtimes/kotlin/undra-runtime/scripts/test-local.sh main)
#   3. compiles the generated bindings (examples/playground/generated/kotlin and examples/two-cores/{a,b}/generated/kotlin)
#      together with the runner (src/)
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

# --- 1. the cores: the host libraries `undra build` writes ---------------------------------------------------------
# The playground core, and S26's two (examples/two-cores/a and b), each rebuilt when missing or older than its sources.
UNDRA="$REPO/target/debug/undra"
host_core() { # <project dir> <namespace>: echoes the library, building it when needed
  local project="$1" ns="$2" lib="" candidate stale=0
  for candidate in "$project/build/host/lib$ns.dylib" "$project/build/host/lib$ns.so"; do
    if [ -f "$candidate" ]; then lib="$candidate"; fi
  done
  if [ -z "$lib" ]; then
    stale=1
  elif [ -n "$(find "$PLAYGROUND/core" "$project/undra.toml" "$REPO/crates" \( -name '*.rs' -o -name 'Cargo.toml' -o -name undra.toml \) -newer "$lib" -print -quit)" ]; then
    stale=1
  fi
  if [ "$stale" = 1 ]; then
    if [ ! -x "$UNDRA" ]; then
      echo "==> building the undra CLI" >&2
      cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli >&2
    fi
    echo "==> undra build -C ${project#"$REPO"/} --platform host" >&2
    "$UNDRA" build -C "$project" --platform host >&2
  fi
  echo "$project/build/host"
}
LIB_DIR="$(host_core "$PLAYGROUND" playground_core)"
LIB_DIR_A="$(host_core "$REPO/examples/two-cores/a" playground_a)"
LIB_DIR_B="$(host_core "$REPO/examples/two-cores/b" playground_b)"
LIB="$LIB_DIR/libplayground_core"

# --- 2. the Kotlin runtime (main sources only) -----------------------------------------------------------
UNDRA_BUILD_DIR="$OUT/runtime" "$REPO/runtimes/kotlin/undra-runtime/scripts/test-local.sh" main

# --- 3. the generated bindings and the runner ------------------------------------------------------------
GENERATED="$PLAYGROUND/generated/kotlin/src/main/kotlin"
GENERATED_A="$REPO/examples/two-cores/a/generated/kotlin/src/main/kotlin"
GENERATED_B="$REPO/examples/two-cores/b/generated/kotlin/src/main/kotlin"
CLASSES="$OUT/classes"
STAMP="$OUT/classes.stamp"
if [ ! -f "$STAMP" ] || [ "$OUT/runtime/main.stamp" -nt "$STAMP" ] \
   || [ -n "$(find "$HERE/src" "$GENERATED" "$GENERATED_A" "$GENERATED_B" -type f -newer "$STAMP" -print -quit)" ]; then
  echo "==> compiling the bindings and the runner"
  rm -rf "$CLASSES" "$STAMP"
  mkdir -p "$CLASSES"
  kotlinc -cp "$OUT/runtime/main:$UNDRA_KOTLINX_COROUTINES" -jvm-target 11 -d "$CLASSES" "$GENERATED" "$GENERATED_A" "$GENERATED_B" "$HERE/src"
  touch "$STAMP"
fi

# --- 4. run, then check the verdicts ------------------------------------------------------------------------
# S19 step 9 replays the derived-list recording (written when missing or stale).
"$REPO/contract-tests/derived-vectors.sh"
export UNDRA_DERIVED_VECTORS="${UNDRA_DERIVED_VECTORS:-$PLAYGROUND/build/derived-vectors.bin}"
echo "==> running S01..S19 and S26 against ${LIB#"$REPO"/} (and playground_a, playground_b)"
mkdir -p "$OUT"
status=0
java -Xmx1g -Djava.library.path="$LIB_DIR:$LIB_DIR_A:$LIB_DIR_B" \
  -cp "$OUT/runtime/main:$CLASSES:$UNDRA_KOTLIN_STDLIB:$UNDRA_KOTLINX_COROUTINES" \
  dev.undra.contract.MainKt 2>&1 | tee "$OUT/run.log" || status=$?
"$REPO/contract-tests/check.sh" kotlin "$OUT/run.log" || status=1
exit "$status"
