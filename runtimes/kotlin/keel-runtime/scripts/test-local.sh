#!/usr/bin/env bash
# Build and test the Kotlin runtime without Gradle or JUnit.
#
# Compiles runtime/src/main and runtime/src/test with the repo's scripts/kotlinc.sh (real `kotlinc`
# if it is on PATH, otherwise the compiler embedded in a Gradle distribution), then runs every test
# suite through the reflection-free runner in TestMain.kt. The tests are the same classes Gradle runs
# under JUnit 5; here `org.junit.jupiter.api.Test` resolves to a stub annotation
# (scripts/local/junit-stub) so nothing needs to be downloaded.
#
#   scripts/test-local.sh                 # everything
#   KEEL_FUZZ_ITERATIONS=50000 scripts/test-local.sh
#   KEEL_FUZZ_SEED=1234 scripts/test-local.sh
#   KEEL_WERROR=0 scripts/test-local.sh   # do not treat compiler warnings as errors
#
# Environment overrides: KEEL_KOTLINX_COROUTINES (coroutines jar), KEEL_KOTLIN_STDLIB (kotlin-stdlib jar),
# GRADLE_HOME (default /opt/gradle).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # runtimes/kotlin/keel-runtime
REPO="$(cd "$HERE/../../.." && pwd)"
KOTLINC="$REPO/scripts/kotlinc.sh"
OUT="$HERE/build/local"
GRADLE_LIB="${GRADLE_HOME:-/opt/gradle}/lib"

# --- locate jars ---------------------------------------------------------------------------------------
COROUTINES="${KEEL_KOTLINX_COROUTINES:-$GRADLE_LIB/kotlinx-coroutines-core-jvm-1.6.4.jar}"
if [ ! -f "$COROUTINES" ]; then
  echo "note: kotlinx-coroutines jar not found at $COROUTINES; compiling without it (the wire layer does not use it)" >&2
  COROUTINES=""
fi

STDLIB="${KEEL_KOTLIN_STDLIB:-}"
if [ -z "$STDLIB" ] && [ -f "$GRADLE_LIB/kotlin-stdlib-2.0.21.jar" ]; then
  STDLIB="$GRADLE_LIB/kotlin-stdlib-2.0.21.jar"
fi
if [ -z "$STDLIB" ] && command -v kotlinc >/dev/null 2>&1; then
  # A real kotlinc install (for example Homebrew): its lib/ directory holds the stdlib.
  KOTLINC_BIN="$(readlink -f "$(command -v kotlinc)" 2>/dev/null || command -v kotlinc)"
  KOTLIN_HOME_GUESS="$(cd "$(dirname "$KOTLINC_BIN")/.." && pwd)"
  if [ -f "$KOTLIN_HOME_GUESS/lib/kotlin-stdlib.jar" ]; then STDLIB="$KOTLIN_HOME_GUESS/lib/kotlin-stdlib.jar"; fi
fi
if [ -z "$STDLIB" ]; then
  echo "error: cannot find kotlin-stdlib; set KEEL_KOTLIN_STDLIB=/path/to/kotlin-stdlib.jar" >&2
  exit 2
fi

FLAGS=(-jvm-target 11)
if [ "${KEEL_WERROR:-1}" = "1" ]; then FLAGS+=(-Werror); fi

# Colon-join the non-empty arguments (bash 3.2 on macOS rejects empty arrays under `set -u`, so no arrays here).
join_cp() {
  local out="" part
  for part in "$@"; do
    if [ -n "$part" ]; then out="${out:+$out:}$part"; fi
  done
  echo "$out"
}

# --- 1. the checked-in vector table must match contract-tests/wire-vectors.json ------------------------
if command -v python3 >/dev/null 2>&1; then
  echo "==> checking WireVectors.kt is up to date"
  python3 "$HERE/scripts/gen-vectors.py" --check
else
  echo "note: python3 not found; skipping the WireVectors.kt freshness check" >&2
fi

# --- 2. every test class must be registered in TestMain.kt ----------------------------------------------
echo "==> checking every Suite is registered in TestMain"
MAIN_FILE="$HERE/runtime/src/test/kotlin/dev/keel/runtime/TestMain.kt"
missing=0
while read -r name; do
  if ! grep -q "$name()" "$MAIN_FILE"; then
    echo "error: $name extends Suite but TestMain.kt does not run it" >&2
    missing=1
  fi
done < <(grep -rhoE 'class [A-Za-z0-9_]+ : Suite\(\)' "$HERE/runtime/src/test/kotlin" | awk '{print $2}')
[ "$missing" = "0" ] || exit 1

# --- 3. compile ------------------------------------------------------------------------------------------
rm -rf "$OUT"
mkdir -p "$OUT/main" "$OUT/test"

echo "==> compiling runtime/src/main (explicit API strict)"
"$KOTLINC" -cp "$(join_cp "$COROUTINES")" -d "$OUT/main" "${FLAGS[@]}" -Xexplicit-api=strict "$HERE/runtime/src/main/kotlin"

echo "==> compiling runtime/src/test"
"$KOTLINC" -cp "$(join_cp "$OUT/main" "$COROUTINES")" -d "$OUT/test" "${FLAGS[@]}" \
  "$HERE/runtime/src/test/kotlin" "$HERE/scripts/local/junit-stub"

# --- 4. run ------------------------------------------------------------------------------------------------
echo "==> running suites"
exec java -Xmx512m -cp "$(join_cp "$OUT/main" "$OUT/test" "$STDLIB" "$COROUTINES")" dev.keel.runtime.TestMainKt
