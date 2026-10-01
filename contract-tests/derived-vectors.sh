#!/usr/bin/env bash
# Writes the recording contract scenario S19 step 9 replays on every platform: 60,000 seeded
# operations over three derived views (ADR-039), checked in Rust against filter + stable sort after
# every operation, as the change-sets a host received and each view's hash after each of them
# (crates/undra-signals/examples/derived_vectors.rs, format UDV1).
#
#   contract-tests/derived-vectors.sh            # writes examples/playground/build/derived-vectors.bin if stale
#
# UNDRA_DERIVED_VECTORS overrides the path (the runners read the same variable).
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${UNDRA_DERIVED_VECTORS:-$root/examples/playground/build/derived-vectors.bin}"
if [ -f "$out" ] && [ -z "$(find "$root/crates/undra-signals" "$root/crates/undra-wire" -type f -name '*.rs' -newer "$out" -print -quit 2>/dev/null)" ]; then
  exit 0
fi
mkdir -p "$(dirname "$out")"
echo "==> writing the derived-list recording for S19 ($out)"
(cd "$root" && cargo run -q --release -p undra-signals --example derived_vectors -- "$out")
