#!/usr/bin/env bash
# Runs the Swift column of the contract scenarios (contract-tests/scenarios.md): the Swift runtime
# over the C ABI, against the real playground core, through the generated bindings.
#
#   contract-tests/swift/run.sh                 all twenty-one (S01 to S20, S26), then the check
#   contract-tests/swift/run.sh --filter ContractScenarios/testS07_streamWithBackpressure
#   contract-tests/swift/run.sh --floor         the same grid against bindings generated for an iOS 15 floor (ADR-045):
#                                               `ObservableObject` stores and `UndraDuration` (the first argument; it may
#                                               be followed by --filter ...)
#
# What it does:
#   1. builds the cores for the host with the undra CLI (`undra build --platform host`, incremental):
#      first build B of the migration steps (`UNDRA_PLAYGROUND_V2=1`, scenarios.md "Two builds"), whose
#      library is copied aside to .build/core-b, then build A, the default (namespace playground_core),
#      and S26's two (examples/two-cores/a and b),
#   2. stages a copy of each lib<namespace>.dylib under .build/core (Package.swift links them and adds
#      the rpath). Each exports one symbol, its table entry (ADR-044), so the three sit side by side;
#      a dynamic library keeps every object file of the core, so its registrations need no flag,
#   3. runs `swift test`. S14 and S15 hand what build A persisted over in .build/migration,
#   4. swaps build B's library into .build/core and runs only `MigrationBuildB` in a second process
#      (`swift test --skip-build`, UNDRA_CONTRACT_PHASE=B), which prints `SCENARIO S14|S15 FAIL` lines
#      if build B's steps fail, then puts build A's library back (also when something fails),
#   5. pipes the `SCENARIO` lines of both processes through contract-tests/check.sh.
# The exit status is non-zero if the tests fail or any of the twenty-one scenarios is not a PASS.
# A filtered run skips step 4 (the build-B steps need the whole of S14 and S15).
set -euo pipefail
FLOOR=0
if [ "${1:-}" = "--floor" ]; then
  FLOOR=1
  shift
fi
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

CORE="$PROJECT/build/host/libplayground_core.dylib"

# Copies a built library to $2/<its name> with an @rpath install name; the copy keeps the test bundle away
# from the build directories.
stage() {
  local name
  name="$(basename "$1")"
  mkdir -p "$2"
  cp "$1" "$2/$name"
  install_name_tool -id "@rpath/$name" "$2/$name"
  codesign --force --sign - "$2/$name" >/dev/null 2>&1
}

# 1. The cores: build B first, kept aside, then build A (what the apps and the bindings are made from).
UNDRA_PLAYGROUND_V2=1 "$UNDRA" build -C "$PROJECT" --platform host >&2
stage "$CORE" "$STAGE_B"
"$UNDRA" build -C "$PROJECT" --platform host >&2
# 2. Build A, staged where Package.swift links it.
stage "$CORE" "$STAGE"
if cmp -s "$STAGE/libplayground_core.dylib" "$STAGE_B/libplayground_core.dylib"; then
  echo "run.sh: build B's library is identical to build A's; UNDRA_PLAYGROUND_V2=1 did not reach the core" >&2
  exit 1
fi
# S26's two cores: the same core (build A) under two more namespaces.
for ns in a b; do
  "$UNDRA" build -C "$REPO/examples/two-cores/$ns" --platform host >&2
  stage "$REPO/examples/two-cores/$ns/build/host/libplayground_$ns.dylib" "$STAGE"
done

# 2b. --floor: bindings for an iOS 15 floor in place of the committed ones. Each is generated beside this package at the
#     depth of Packages/ (the generated Package.swift names the runtime relative to where it is written, and the symlink
#     in PackagesFloor/ must sit as deep), from the same cores; the committed bindings are not touched.
if [ "$FLOOR" = 1 ]; then
  rm -rf "$HERE/PackagesFloor" "$HERE"/floor-*
  mkdir -p "$HERE/PackagesFloor"
  for entry in "PlaygroundCore:$PROJECT" "PlaygroundA:$REPO/examples/two-cores/a" "PlaygroundB:$REPO/examples/two-cores/b"; do
    name="${entry%%:*}"
    project="${entry#*:}"
    "$UNDRA" bindgen -C "$project" --out "$HERE/floor-$name" --platforms ios \
      --swift-observation observable-object --ios-deployment-target 15.0 >&2
    ln -s "../floor-$name/swift" "$HERE/PackagesFloor/$name"
  done
  export UNDRA_SWIFT_FLOOR=1
  echo "run.sh: the scenarios run against bindings generated for iOS 15.0 (ObservableObject stores, UndraDuration)" >&2
fi

# 3. The scenarios. The runner prints one `SCENARIO Sxx PASS|FAIL|SKIP <title>` line each. S19 step 9
#    replays the derived-list recording (written when missing or stale).
"$REPO/contract-tests/derived-vectors.sh" >&2
export UNDRA_DERIVED_VECTORS="${UNDRA_DERIVED_VECTORS:-$PROJECT/build/derived-vectors.bin}"
cd "$HERE"
rm -rf "$HANDOVER"
export UNDRA_CONTRACT_HANDOVER="$HANDOVER"
status=0
swift test --skip TestKitTests "$@" 2>&1 | tee "$LOG" || status=$?

# 4. Build B's steps, in a second process over build B's library. Build A's goes back afterwards,
# whatever happens, so the next run (and the Package's link) finds it.
if [ "$#" -eq 0 ]; then
  cp "$STAGE/libplayground_core.dylib" "$STAGE/libplayground_core.dylib.a"
  restore_a() { mv -f "$STAGE/libplayground_core.dylib.a" "$STAGE/libplayground_core.dylib"; }
  trap restore_a EXIT
  cp "$STAGE_B/libplayground_core.dylib" "$STAGE/libplayground_core.dylib"
  UNDRA_CONTRACT_PHASE=B swift test --skip-build --filter MigrationBuildB 2>&1 | tee -a "$LOG" || status=$?
  restore_a
  trap - EXIT
fi

# 5. The grid. A filtered run reports the scenarios it did not run as MISSING, which is expected.
"$REPO/contract-tests/check.sh" swift < "$LOG" || status=1

# 5. The testing kit against the same core (its own process: a process holds one in-process core). Skipped when the caller filtered the run.
if [ "$#" = 0 ]; then
  echo "==> the testing kit (PreviewCore, RecordedCore) against the real core" >&2
  swift test --filter TestKitTests 2>&1 | tee "$HERE/.build/testkit.log" || status=1
fi
exit "$status"
