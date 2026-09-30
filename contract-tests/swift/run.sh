#!/usr/bin/env bash
# Runs the Swift column of the contract scenarios (contract-tests/scenarios.md): the Swift runtime
# over the C ABI, against the real playground core, through the generated bindings.
#
#   contract-tests/swift/run.sh                 all seventeen, then the check
#   contract-tests/swift/run.sh --filter ContractScenarios/testS07_streamWithBackpressure
#
# What it does:
#   1. builds the core for the host with the undra CLI (`undra build --platform host`, incremental),
#   2. stages a copy of libundra_core.dylib under .build/core whose install name is
#      `@rpath/libundra_core.dylib` (Package.swift links it and adds the rpath),
#   3. runs `swift test` with UNDRA_LINK_CORE=1: that switch makes the runtime package leave out
#      undra_stub.c, the link-time stand-in for the core, so the `undra_*` symbols are the real ones.
#      A dynamic library (not the static one the apps link) keeps the linker from dropping the
#      object files whose constructors register the core's items, so no -force_load is needed,
#   4. pipes the `SCENARIO` lines through contract-tests/check.sh.
# The exit status is non-zero if the tests fail or any of the seventeen scenarios is not a PASS.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
PROJECT="$REPO/examples/playground"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
LOG="$HERE/.build/contract.log"

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi

if [ ! -x "$UNDRA" ]; then
  cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
fi

# 1. The core.
"$UNDRA" build -C "$PROJECT" --platform host >&2
CORE="$PROJECT/build/host/libundra_core.dylib"

# 2. The staged copy. `undra build` leaves the install name pointing into target/, which would make
# the test bundle depend on that path; a copy with an @rpath name is found through the rpath.
STAGE="$HERE/.build/core"
mkdir -p "$STAGE"
if [ ! -f "$STAGE/libundra_core.dylib" ] || [ "$CORE" -nt "$STAGE/libundra_core.dylib" ]; then
  cp "$CORE" "$STAGE/libundra_core.dylib"
  install_name_tool -id "@rpath/libundra_core.dylib" "$STAGE/libundra_core.dylib"
  codesign --force --sign - "$STAGE/libundra_core.dylib" >/dev/null 2>&1
fi

# 3. The scenarios. The runner prints one `SCENARIO Sxx PASS|FAIL|SKIP <title>` line each.
cd "$HERE"
status=0
UNDRA_LINK_CORE=1 swift test "$@" 2>&1 | tee "$LOG" || status=$?

# 4. The grid. A filtered run reports the scenarios it did not run as MISSING, which is expected.
"$REPO/contract-tests/check.sh" swift < "$LOG" || status=1
exit "$status"
