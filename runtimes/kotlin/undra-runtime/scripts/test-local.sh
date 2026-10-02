#!/usr/bin/env bash
# Build and test the Kotlin runtime without Gradle or JUnit.
#
# Compiles runtime/src/main and runtime/src/test with `kotlinc` from PATH (scripts/env.sh puts the local
# one there and sets UNDRA_KOTLINX_COROUTINES), then runs every test suite through the reflection-free
# runner in TestMain.kt. The tests are the same classes Gradle runs under JUnit 5; here
# `org.junit.jupiter.api.Test` resolves to a stub annotation (scripts/local/junit-stub) so nothing needs
# to be downloaded.
#
# The test build also compiles the generated Kotlin of the bindgen `full` golden case
# (crates/undra-bindgen/tests/golden/full/kotlin) and its execution test
# (crates/undra-bindgen/tests/fixtures/kotlin-run/full) against the real runtime; GoldenFullTests runs it.
#
#   scripts/test-local.sh                 # everything
#   scripts/test-local.sh check|main|test|run   # one phase (each phase is incremental, so a slow machine
#                                               # or a per-command time limit can run them one by one)
#   UNDRA_FUZZ_ITERATIONS=50000 scripts/test-local.sh
#   UNDRA_FUZZ_SEED=1234 scripts/test-local.sh
#   UNDRA_WERROR=0 scripts/test-local.sh   # do not treat compiler warnings as errors
#   UNDRA_FORCE=1 scripts/test-local.sh    # ignore the up-to-date stamps and recompile
#   UNDRA_SKIP_GOLDEN=1 scripts/test-local.sh   # leave the generated golden/full code out of the test build
#   UNDRA_BUILD_DIR=/tmp/undra-kotlin scripts/test-local.sh   # put the build output elsewhere (default: build/local)
#   UNDRA_NATIVE_LIB_DIR=crates/undra-ffi/tests/fixture/target/debug scripts/test-local.sh run
#                                        # -Djava.library.path: where NativeSmokeTests finds libundra_fixture (ADR-044)
#   UNDRA_NATIVE_PATHS="undra_fixture=/abs/libundra_fixture.dylib" scripts/test-local.sh run
#                                        # space-separated namespace=file pairs, each -Dundra.native.<namespace>.path=<file>
#
# Environment: UNDRA_KOTLINX_COROUTINES (kotlinx-coroutines-core-jvm jar; scripts/env.sh sets it),
# UNDRA_KOTLIN_STDLIB (kotlin-stdlib jar; default: the one inside the kotlinc install), UNDRA_SQLITE_JDBC (the SQLite JDBC
# driver jar, put on the class path of the test run only: :runtime itself depends on nothing; JdbcDbAdapterTests skips
# without it, or fails with UNDRA_REQUIRE_TOOLCHAINS=1).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # runtimes/kotlin/undra-runtime
REPO="$(cd "$HERE/../../.." && pwd)"
OUT="${UNDRA_BUILD_DIR:-$HERE/build/local}"
PHASE="${1:-all}"

# --- toolchain -------------------------------------------------------------------------------------------
if ! command -v kotlinc >/dev/null 2>&1 && [ -f "$REPO/scripts/env.sh" ]; then
  # shellcheck disable=SC1091
  source "$REPO/scripts/env.sh"
fi
if ! command -v kotlinc >/dev/null 2>&1; then
  echo "error: kotlinc is not on PATH (source scripts/env.sh, or install Kotlin 2.x)" >&2
  exit 2
fi
COROUTINES="${UNDRA_KOTLINX_COROUTINES:-}"
if [ -z "$COROUTINES" ] || [ ! -f "$COROUTINES" ]; then
  echo "error: set UNDRA_KOTLINX_COROUTINES to kotlinx-coroutines-core-jvm-1.6.4.jar (scripts/env.sh does)" >&2
  exit 2
fi
STDLIB="${UNDRA_KOTLIN_STDLIB:-}"
if [ -z "$STDLIB" ]; then
  # A kotlinc install keeps kotlin-stdlib.jar in lib/, next to bin/.
  KOTLINC_BIN="$(readlink -f "$(command -v kotlinc)" 2>/dev/null || command -v kotlinc)"
  KOTLIN_HOME_GUESS="$(cd "$(dirname "$KOTLINC_BIN")/.." && pwd)"
  if [ -f "$KOTLIN_HOME_GUESS/lib/kotlin-stdlib.jar" ]; then STDLIB="$KOTLIN_HOME_GUESS/lib/kotlin-stdlib.jar"; fi
fi
if [ -z "$STDLIB" ]; then
  echo "error: cannot find kotlin-stdlib; set UNDRA_KOTLIN_STDLIB=/path/to/kotlin-stdlib.jar" >&2
  exit 2
fi

FLAGS=(-jvm-target 11)
if [ "${UNDRA_WERROR:-1}" = "1" ]; then FLAGS+=(-Werror); fi

# Colon-join the non-empty arguments (bash 3.2 on macOS rejects empty arrays under `set -u`, so no arrays of paths).
join_cp() {
  local out="" part
  for part in "$@"; do
    if [ -n "$part" ]; then out="${out:+$out:}$part"; fi
  done
  echo "$out"
}

MAIN_SRC="$HERE/runtime/src/main/kotlin"
TEST_SRC="$HERE/runtime/src/test/kotlin"
# Test support shared with android-adapters' tests (FaultyFileSystem), compiled into the test build.
SUPPORT_SRC="$HERE/test-support/kotlin"
GOLDEN_SRC="$REPO/crates/undra-bindgen/tests/golden/full/kotlin/src/main/kotlin"
GOLDEN_RUN="$REPO/crates/undra-bindgen/tests/fixtures/kotlin-run/full"

# up_to_date STAMP DIR... : true when STAMP exists and no file under the DIRs is newer than it.
up_to_date() {
  local stamp="$1"; shift
  [ "${UNDRA_FORCE:-0}" = "1" ] && return 1
  [ -f "$stamp" ] || return 1
  local dir
  for dir in "$@"; do
    [ -d "$dir" ] || continue
    if [ -n "$(find "$dir" -type f -newer "$stamp" -print -quit)" ]; then return 1; fi
  done
  return 0
}

# --- 1. checks -------------------------------------------------------------------------------------------
phase_check() {
  # The checked-in vector table must match contract-tests/wire-vectors.json.
  if command -v python3 >/dev/null 2>&1; then
    echo "==> checking WireVectors.kt is up to date"
    python3 "$HERE/scripts/gen-vectors.py" --check
  else
    echo "note: python3 not found; skipping the WireVectors.kt freshness check" >&2
  fi

  # Every test class must be registered in TestMain.kt.
  echo "==> checking every Suite is registered in TestMain"
  local main_file="$TEST_SRC/dev/undra/runtime/TestMain.kt" missing=0 name
  while read -r name; do
    if ! grep -q "$name()" "$main_file"; then
      echo "error: $name extends Suite but TestMain.kt does not run it" >&2
      missing=1
    fi
  done < <(grep -rhoE 'class [A-Za-z0-9_]+ : Suite\(\)' "$TEST_SRC" | awk '{print $2}')
  [ "$missing" = "0" ] || exit 1
}

# --- 2. compile ------------------------------------------------------------------------------------------
phase_main() {
  if up_to_date "$OUT/main.stamp" "$MAIN_SRC"; then
    echo "==> runtime/src/main is up to date"
    return
  fi
  rm -rf "$OUT/main" "$OUT/main.stamp"
  mkdir -p "$OUT/main"
  echo "==> compiling runtime/src/main (explicit API strict)"
  kotlinc -cp "$(join_cp "$COROUTINES")" -d "$OUT/main" "${FLAGS[@]}" -Xexplicit-api=strict "$MAIN_SRC"
  touch "$OUT/main.stamp"
}

phase_test() {
  phase_main
  if up_to_date "$OUT/test.stamp" "$TEST_SRC" "$SUPPORT_SRC" "$MAIN_SRC" "$HERE/scripts/local" "$GOLDEN_SRC" "$GOLDEN_RUN" \
     && [ "$OUT/test.stamp" -nt "$OUT/main.stamp" ]; then
    echo "==> runtime/src/test is up to date"
    return
  fi
  rm -rf "$OUT/test" "$OUT/test.stamp"
  mkdir -p "$OUT/test"
  local sources=("$TEST_SRC" "$SUPPORT_SRC" "$HERE/scripts/local/junit-stub")
  if [ "${UNDRA_SKIP_GOLDEN:-0}" = "1" ]; then
    echo "note: UNDRA_SKIP_GOLDEN=1; GoldenFullTests will be skipped" >&2
  elif [ -d "$GOLDEN_SRC" ] && [ -d "$GOLDEN_RUN" ]; then
    sources+=("$GOLDEN_SRC" "$GOLDEN_RUN")
  else
    echo "note: bindgen golden sources not found; GoldenFullTests will be skipped" >&2
  fi
  echo "==> compiling runtime/src/test (+ generated golden/full)"
  # -Xfriend-paths lets the tests see `internal` declarations of the main module, as Gradle does.
  kotlinc -cp "$(join_cp "$OUT/main" "$COROUTINES")" -d "$OUT/test" "${FLAGS[@]}" \
    -Xfriend-paths="$OUT/main" "${sources[@]}"
  touch "$OUT/test.stamp"
}

# --- 3. run ----------------------------------------------------------------------------------------------
phase_run() {
  phase_test
  echo "==> running suites"
  local jflags=(-Xmx512m)
  local pair
  if [ -n "${UNDRA_NATIVE_NAME:-}${UNDRA_NATIVE_PATH:-}" ]; then
    echo "warning: UNDRA_NATIVE_NAME / UNDRA_NATIVE_PATH are gone (ADR-044: a core's library is lib<namespace>);" \
      "NativeSmokeTests loads undra-ffi's fixture core: UNDRA_NATIVE_LIB_DIR=<dir of libundra_fixture> or" \
      "UNDRA_NATIVE_PATHS=\"undra_fixture=<file>\"" >&2
  fi
  if [ -n "${UNDRA_NATIVE_LIB_DIR:-}" ]; then jflags+=("-Djava.library.path=$UNDRA_NATIVE_LIB_DIR"); fi
  for pair in ${UNDRA_NATIVE_PATHS:-}; do
    case "$pair" in
      *=*) jflags+=("-Dundra.native.${pair%%=*}.path=${pair#*=}") ;;
      *) echo "error: UNDRA_NATIVE_PATHS takes namespace=file pairs, got: $pair" >&2; exit 2 ;;
    esac
  done
  local driver="${UNDRA_SQLITE_JDBC:-}"
  if [ -n "$driver" ] && [ ! -f "$driver" ]; then
    echo "error: UNDRA_SQLITE_JDBC=$driver is not a file" >&2
    exit 2
  fi
  # runtime/src/main/resources is on the classpath as in the jar (the R8 consumer rules, META-INF/proguard).
  java "${jflags[@]}" -cp "$(join_cp "$OUT/main" "$HERE/runtime/src/main/resources" "$OUT/test" "$STDLIB" "$COROUTINES" "$driver")" \
    dev.undra.runtime.TestMainKt
}

case "$PHASE" in
  check) phase_check ;;
  main)  phase_main ;;
  test)  phase_test ;;
  run)   phase_run ;;
  all)   phase_check; phase_run ;;
  *) echo "usage: test-local.sh [all|check|main|test|run]" >&2; exit 2 ;;
esac
