#!/usr/bin/env bash
# Runs the Swift column of the contract scenarios (contract-tests/scenarios.md): the Swift runtime
# over the C ABI, against the real playground core, through the generated bindings.
#
#   contract-tests/swift/run.sh                 all nineteen (S01 to S19), then the check
#   contract-tests/swift/run.sh --filter ContractScenarios/testS07_streamWithBackpressure
#
# What it does:
#   1. builds the core for the host twice with the undra CLI (`undra build --platform host`,
#      incremental): first build B of the migration steps (`UNDRA_PLAYGROUND_V2=1`, scenarios.md
#      "Two builds"), whose library is copied aside to .build/core-b, then build A, the default,
#   2. stages a copy of build A's libundra_core.dylib under .build/core whose install name is
#      `@rpath/libundra_core.dylib` (Package.swift links it and adds the rpath),
#   3. runs `swift test` with UNDRA_LINK_CORE=1: that switch makes the runtime package leave out
#      undra_stub.c, the link-time stand-in for the core, so the `undra_*` symbols are the real ones.
#      A dynamic library (not the static one the apps link) keeps the linker from dropping the
#      object files whose constructors register the core's items, so no -force_load is needed.
#      S14 and S15 hand what build A persisted over in .build/migration,
#   4. swaps build B's library into .build/core and runs only `MigrationBuildB` in a second process
#      (`swift test --skip-build`, UNDRA_CONTRACT_PHASE=B), which prints `SCENARIO S14|S15 FAIL` lines
#      if build B's steps fail, then puts build A's library back (also when something fails),
#   5. pipes the `SCENARIO` lines of both processes through contract-tests/check.sh.
# The exit status is non-zero if the tests fail or any of the nineteen scenarios is not a PASS.
# A filtered run skips step 4 (the build-B steps need the whole of S14 and S15).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
PROJECT="$REPO/examples/playground"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
LOG="$HERE/.build/contract.log"
STAGE="$HERE/.build/core"
STAGE_B="$HERE/.build/core-b"
HANDOVER="$HERE/.build/migration"

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi

if [ ! -x "$UNDRA" ]; then
  cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
fi

CORE="$PROJECT/build/host/libundra_core.dylib"

# Copies a built library to $2/libundra_core.dylib with an @rpath install name (`undra build` leaves
# it pointing into target/, which would make the test bundle depend on that path).
stage() {
  mkdir -p "$2"
  cp "$1" "$2/libundra_core.dylib"
  install_name_tool -id "@rpath/libundra_core.dylib" "$2/libundra_core.dylib"
  codesign --force --sign - "$2/libundra_core.dylib" >/dev/null 2>&1
}

# 1. The cores: build B first, kept aside, then build A (what the apps and the bindings are made from).
UNDRA_PLAYGROUND_V2=1 "$UNDRA" build -C "$PROJECT" --platform host >&2
stage "$CORE" "$STAGE_B"
"$UNDRA" build -C "$PROJECT" --platform host >&2
# 2. Build A, staged where Package.swift links it.
stage "$CORE" "$STAGE"
if cmp -s "$STAGE/libundra_core.dylib" "$STAGE_B/libundra_core.dylib"; then
  echo "run.sh: build B's library is identical to build A's; UNDRA_PLAYGROUND_V2=1 did not reach the core" >&2
  exit 1
fi

# 3. The scenarios. The runner prints one `SCENARIO Sxx PASS|FAIL|SKIP <title>` line each.
cd "$HERE"
rm -rf "$HANDOVER"
export UNDRA_CONTRACT_HANDOVER="$HANDOVER"
status=0
UNDRA_LINK_CORE=1 swift test "$@" 2>&1 | tee "$LOG" || status=$?

# 4. Build B's steps, in a second process over build B's library. Build A's goes back afterwards,
# whatever happens, so the next run (and the Package's link) finds it.
if [ "$#" -eq 0 ]; then
  cp "$STAGE/libundra_core.dylib" "$STAGE/libundra_core.dylib.a"
  restore_a() { mv -f "$STAGE/libundra_core.dylib.a" "$STAGE/libundra_core.dylib"; }
  trap restore_a EXIT
  cp "$STAGE_B/libundra_core.dylib" "$STAGE/libundra_core.dylib"
  UNDRA_CONTRACT_PHASE=B UNDRA_LINK_CORE=1 swift test --skip-build --filter MigrationBuildB 2>&1 | tee -a "$LOG" || status=$?
  restore_a
  trap - EXIT
fi

# 5. The grid. A filtered run reports the scenarios it did not run as MISSING, which is expected.
"$REPO/contract-tests/check.sh" swift < "$LOG" || status=1
exit "$status"
