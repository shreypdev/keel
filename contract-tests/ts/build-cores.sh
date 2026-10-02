#!/usr/bin/env bash
# Builds the wasm cores the TypeScript column of the contract scenarios (contract-tests/scenarios.md) and every runner
# that reuses its scenarios over the same cores (the React Native column, runtimes/rn/@undra/react-native, `npm run
# test:contract`) load, each when it is missing or older than its sources:
#
#   * build A of the playground core, examples/playground/build/web/playground_core.wasm (the default build);
#   * build B of it (UNDRA_PLAYGROUND_V2=1, scenarios.md "Two builds": S14 steps 7 to 9, S15 steps 11 to 14), copied to
#     contract-tests/ts/build/b/playground_core.wasm (not committed), because the build directory holds one build at a time;
#   * the same core under the namespaces of S26 and S27, examples/two-cores/{a,b}/build/web/playground_{a,b}.wasm.
#
#   contract-tests/ts/build-cores.sh
#
# None of these is committed, so a fresh checkout has none of them: run.sh calls this before the scenarios, and a runner of
# the same scenarios that does not go through run.sh calls it itself (the CI job of the React Native column does).
# UNDRA_PLAYGROUND_WASM and UNDRA_PLAYGROUND_WASM_B point at builds made elsewhere (then they are not rebuilt);
# UNDRA_CLI overrides the path of the undra binary (default target/debug/undra, built if missing).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

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
  echo "build-cores.sh: build B's wasm ($wasm_b) is identical to build A's ($wasm_a); UNDRA_PLAYGROUND_V2=1 did not reach the core" >&2
  exit 1
fi
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
