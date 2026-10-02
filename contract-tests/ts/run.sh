#!/usr/bin/env bash
# The TypeScript column of the contract scenarios (contract-tests/scenarios.md): @undra/runtime over
# the real wasm build of the playground core, in wasm-main mode (and wasm-worker where a scenario says so),
# under vitest on Node.
#
#   contract-tests/ts/run.sh              # build the cores if missing or stale, run S01..S28 and S31..S33, grade
#   contract-tests/ts/run.sh -t S07       # extra arguments go to vitest (here: only scenario S07)
#
# Builds with the undra CLI (`undra build -C examples/playground --platform web`, which writes
# examples/playground/build/web/playground_core.wasm) unless UNDRA_PLAYGROUND_WASM points somewhere else,
# and the same core under the namespaces of S26 (examples/two-cores/{a,b}/build/web/playground_{a,b}.wasm).
# Build B of the two-build steps (scenarios.md, "Two builds": S14 steps 7 to 9, S15 steps 11 to 14) is the
# same command with UNDRA_PLAYGROUND_V2=1; its wasm is copied to build/b/playground_core.wasm (not committed) before
# build A is built again, so the default wasm stays build A. UNDRA_PLAYGROUND_WASM_B overrides its path.
# Prints `SCENARIO Sxx PASS|FAIL|SKIP <title>` lines (src/reporter.ts) and pipes them through
# contract-tests/check.sh, so the exit status is non-zero unless every one (S01..S28 and S31..S33) passes.
# UNDRA_CLI overrides the path of the undra binary (default target/debug/undra, built if missing).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
cd "$here"

undra="${UNDRA_CLI:-$root/target/debug/undra}"
ensure_cli() {
  if [ ! -x "$undra" ]; then
    echo "==> building the undra CLI" >&2
    (cd "$root" && cargo build -p undra-cli) >&2
  fi
}
# Whether $1 is missing or older than a source the core is built from ($2: the project's undra.toml).
stale() {
  [ ! -f "$1" ] && return 0
  [ -n "$(find "$root/examples/playground/core" "$root/crates" "$root/Cargo.toml" "$root/Cargo.lock" "${2:-$root/examples/playground/undra.toml}" \
        \( -name target -o -name node_modules \) -prune -o \
        -type f \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock -o -name undra.toml \) -newer "$1" -print -quit 2>/dev/null)" ]
}

# 1. The cores as wasm: build B first (kept aside), then build A, each built when missing or stale.
built="$root/examples/playground/build/web/playground_core.wasm"
wasm_a="${UNDRA_PLAYGROUND_WASM:-$built}"
wasm_b="${UNDRA_PLAYGROUND_WASM_B:-$here/build/b/playground_core.wasm}"
rebuild_a=0
if [ -z "${UNDRA_PLAYGROUND_WASM_B:-}" ] && stale "$wasm_b"; then
  ensure_cli
  echo "==> building build B of the playground core for web (UNDRA_PLAYGROUND_V2=1)" >&2
  UNDRA_PLAYGROUND_V2=1 "$undra" build -C "$root/examples/playground" --platform web >&2
  mkdir -p "$(dirname "$wasm_b")"
  cp "$built" "$wasm_b"
  # The build directory now holds build B: build A goes back in below.
  rebuild_a=1
fi
if [ -z "${UNDRA_PLAYGROUND_WASM:-}" ] && { [ "$rebuild_a" = 1 ] || stale "$wasm_a"; }; then
  ensure_cli
  echo "==> building the playground core for web" >&2
  "$undra" build -C "$root/examples/playground" --platform web >&2
fi
if cmp -s "$wasm_a" "$wasm_b"; then
  echo "run.sh: build B's wasm ($wasm_b) is identical to build A's ($wasm_a); UNDRA_PLAYGROUND_V2=1 did not reach the core" >&2
  exit 1
fi
export UNDRA_PLAYGROUND_WASM_B="$wasm_b"
# The same core (build A) under the two namespaces of S26 (ADR-044).
for ns in a b; do
  project="$root/examples/two-cores/$ns"
  wasm="$project/build/web/playground_$ns.wasm"
  if stale "$wasm" "$project/undra.toml"; then
    ensure_cli
    echo "==> building $(basename "$wasm") for web" >&2
    "$undra" build -C "$project" --platform web >&2
  fi
done

# 1b. The derived-list recording S19 replays (crates/undra-signals/examples/derived_vectors.rs), written
#     when it is missing or older than the signals crate it records.
"$root/contract-tests/derived-vectors.sh" >&2

# 2. The dependencies (vitest, typescript), from the lockfile.
#    Never through a symlink: `npm ci` empties the directory a linked `node_modules` points at (another
#    checkout's, typically a worktree's link to the main checkout's), so a link that needs an install is refused.
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
  if [ -L node_modules ]; then
    echo "run.sh: node_modules is a symlink (to $(readlink node_modules)) and the lockfile needs an install;" >&2
    echo "        refusing to run npm ci through it. Remove the link and rerun (npm ci installs a copy here)," >&2
    echo "        or link a node_modules installed from this package-lock.json." >&2
    exit 2
  fi
  echo "==> installing dependencies" >&2
  npm ci --no-audit --no-fund >&2
fi

# 3. The scenarios, then the grade.
log="$here/run.log"
status=0
npx vitest run "$@" 2>&1 | tee "$log" || status=$?
"$root/contract-tests/check.sh" ts "$log" || status=$?
exit "$status"
