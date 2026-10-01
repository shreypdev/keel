#!/usr/bin/env bash
# The TypeScript column of the contract scenarios (contract-tests/scenarios.md): @undra/runtime over
# the real wasm build of the playground core, in wasm-main mode, under vitest on Node.
#
#   contract-tests/ts/run.sh              # build the core if it is missing or stale, run S01..S18 and S23..S25, grade
#   contract-tests/ts/run.sh -t S07       # extra arguments go to vitest (here: only scenario S07)
#
# Builds with the undra CLI (`undra build -C examples/playground --platform web`, which writes
# examples/playground/build/web/undra_core.wasm) unless UNDRA_PLAYGROUND_WASM points somewhere else.
# Prints `SCENARIO Sxx PASS|FAIL|SKIP <title>` lines (src/reporter.ts) and pipes them through
# contract-tests/check.sh, so the exit status is non-zero unless all twenty-one pass.
# UNDRA_CLI overrides the path of the undra binary (default target/debug/undra, built if missing).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
cd "$here"

# 1. The core as wasm: built when the file is missing or older than a source it is built from.
if [ -z "${UNDRA_PLAYGROUND_WASM:-}" ]; then
  wasm="$root/examples/playground/build/web/undra_core.wasm"
  undra="${UNDRA_CLI:-$root/target/debug/undra}"
  stale=0
  if [ ! -f "$wasm" ]; then
    stale=1
  elif [ -n "$(find "$root/examples/playground/core" "$root/crates" "$root/Cargo.toml" "$root/Cargo.lock" \
        \( -name target -o -name node_modules \) -prune -o \
        -type f \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) -newer "$wasm" -print -quit 2>/dev/null)" ]; then
    stale=1
  fi
  if [ "$stale" = 1 ]; then
    if [ ! -x "$undra" ]; then
      echo "==> building the undra CLI" >&2
      (cd "$root" && cargo build -p undra-cli) >&2
    fi
    echo "==> building the playground core for web" >&2
    "$undra" build -C "$root/examples/playground" --platform web >&2
  fi
fi

# 2. The dependencies (vitest, typescript), from the lockfile.
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
  echo "==> installing dependencies" >&2
  npm ci --no-audit --no-fund >&2
fi

# 3. The scenarios, then the grade.
log="$here/run.log"
status=0
npx vitest run "$@" 2>&1 | tee "$log" || status=$?
"$root/contract-tests/check.sh" ts "$log" || status=$?
exit "$status"
