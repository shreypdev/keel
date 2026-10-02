#!/usr/bin/env bash
# The TypeScript column of the contract scenarios (contract-tests/scenarios.md): @undra/runtime over
# the real wasm build of the playground core, in wasm-main mode (and wasm-worker where a scenario says so),
# under vitest on Node.
#
#   contract-tests/ts/run.sh              # build the cores if missing or stale, run S01..S28 and S31..S33, grade
#   contract-tests/ts/run.sh -t S07       # extra arguments go to vitest (here: only scenario S07)
#
# Builds the cores first (contract-tests/ts/build-cores.sh, which a runner of the same scenarios that does not go
# through this script calls itself): the undra CLI's `undra build -C examples/playground --platform web`, which writes
# examples/playground/build/web/playground_core.wasm, unless UNDRA_PLAYGROUND_WASM points somewhere else, and the same
# core under the namespaces of S26 and S27 (examples/two-cores/{a,b}/build/web/playground_{a,b}.wasm).
# Build B of the two-build steps (scenarios.md, "Two builds": S14 steps 7 to 9, S15 steps 11 to 14) is the
# same command with UNDRA_PLAYGROUND_V2=1; its wasm is copied to build/b/playground_core.wasm (not committed) before
# build A is built again, so the default wasm stays build A. UNDRA_PLAYGROUND_WASM_B overrides its path.
# The dependencies (the runtime's and this directory's) are installed with `npm ci` when missing or older than their lockfiles.
# Prints `SCENARIO Sxx PASS|FAIL|SKIP <title>` lines (src/reporter.ts) and pipes them through
# contract-tests/check.sh, so the exit status is non-zero unless every one (S01..S33) passes.
# UNDRA_CLI overrides the path of the undra binary (default target/debug/undra, built if missing).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
cd "$here"

# 1. The cores as wasm: builds A and B of the playground core and the two-cores pair, each built when missing or stale.
"$here/build-cores.sh"
export UNDRA_PLAYGROUND_WASM_B="${UNDRA_PLAYGROUND_WASM_B:-$here/build/b/playground_core.wasm}"

# 1b. The derived-list recording S19 replays (crates/undra-signals/examples/derived_vectors.rs), written
#     when it is missing or older than the signals crate it records.
"$root/contract-tests/derived-vectors.sh" >&2

# 2. The dependencies (vitest, typescript), from the lockfile, here and in the runtime.
#    The runner uses the runtime's sources (the aliases of vitest.config.ts), so what those sources import (S25 imports
#    wa-sqlite through `@undra/runtime/db-worker`) is resolved from the runtime's own `node_modules`.
#    Never through a symlink: `npm ci` empties the directory a linked `node_modules` points at (another
#    checkout's, typically a worktree's link to the main checkout's), so a link that needs an install is refused.
install_dependencies() { # <package directory>
  local dir="$1"
  if [ ! -d "$dir/node_modules" ] || [ "$dir/package-lock.json" -nt "$dir/node_modules/.package-lock.json" ]; then
    if [ -L "$dir/node_modules" ]; then
      echo "run.sh: $dir/node_modules is a symlink (to $(readlink "$dir/node_modules")) and the lockfile needs an install;" >&2
      echo "        refusing to run npm ci through it. Remove the link and rerun (npm ci installs a copy here)," >&2
      echo "        or link a node_modules installed from this package-lock.json." >&2
      exit 2
    fi
    echo "==> installing dependencies of ${dir#"$root"/}" >&2
    (cd "$dir" && npm ci --no-audit --no-fund >&2)
  fi
}
install_dependencies "$root/runtimes/ts/@undra/runtime"
install_dependencies "$here"

# 3. The scenarios, then the grade.
log="$here/run.log"
status=0
npx vitest run "$@" 2>&1 | tee "$log" || status=$?
"$root/contract-tests/check.sh" ts "$log" || status=$?
exit "$status"
