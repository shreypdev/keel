#!/usr/bin/env bash
# Runs the Swift column of the contract scenarios (contract-tests/scenarios.md): the Swift runtime
# over the C ABI, against the real playground core, through the generated bindings.
#
#   contract-tests/swift/run.sh                 all twenty, then the check
#   contract-tests/swift/run.sh --filter ContractScenarios/testS07_streamWithBackpressure
#
# What it does:
#   1. builds the cores for the host with the undra CLI (`undra build --platform host`, incremental):
#      the playground's (namespace playground_core) and S26's two (examples/two-cores/a and b),
#   2. stages a copy of each lib<namespace>.dylib under .build/core (Package.swift links them and adds
#      the rpath). Each exports one symbol, its table entry (ADR-044), so the three sit side by side;
#      a dynamic library keeps every object file of the core, so its registrations need no flag,
#   3. runs `swift test`,
#   4. pipes the `SCENARIO` lines through contract-tests/check.sh.
# The exit status is non-zero if the tests fail or any of the twenty scenarios is not a PASS.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
LOG="$HERE/.build/contract.log"

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi

if [ ! -x "$UNDRA" ]; then
  cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
fi

# 1 and 2. The cores and their staged copies. `undra build` names each `@rpath/lib<namespace>.dylib`;
# the copy keeps the test bundle away from the build directories.
STAGE="$HERE/.build/core"
mkdir -p "$STAGE"
stage() { # <project dir> <namespace>
  "$UNDRA" build -C "$1" --platform host >&2
  local core="$1/build/host/lib$2.dylib"
  if [ ! -f "$STAGE/lib$2.dylib" ] || [ "$core" -nt "$STAGE/lib$2.dylib" ]; then
    cp "$core" "$STAGE/lib$2.dylib"
    install_name_tool -id "@rpath/lib$2.dylib" "$STAGE/lib$2.dylib"
    codesign --force --sign - "$STAGE/lib$2.dylib" >/dev/null 2>&1
  fi
}
stage "$REPO/examples/playground" playground_core
stage "$REPO/examples/two-cores/a" playground_a
stage "$REPO/examples/two-cores/b" playground_b

# 3. The scenarios. The runner prints one `SCENARIO Sxx PASS|FAIL|SKIP <title>` line each. S19 step 9
#    replays the derived-list recording (written when missing or stale).
"$REPO/contract-tests/derived-vectors.sh" >&2
export UNDRA_DERIVED_VECTORS="${UNDRA_DERIVED_VECTORS:-$REPO/examples/playground/build/derived-vectors.bin}"
cd "$HERE"
status=0
swift test --skip TestKitTests "$@" 2>&1 | tee "$LOG" || status=$?

# 4. The grid. A filtered run reports the scenarios it did not run as MISSING, which is expected.
"$REPO/contract-tests/check.sh" swift < "$LOG" || status=1

# 5. The testing kit against the same core (its own process: a process holds one in-process core). Skipped when the caller filtered the run.
if [ "$#" = 0 ]; then
  echo "==> the testing kit (PreviewCore, RecordedCore) against the real core" >&2
  UNDRA_LINK_CORE=1 swift test --filter TestKitTests 2>&1 | tee "$HERE/.build/testkit.log" || status=1
fi
exit "$status"
