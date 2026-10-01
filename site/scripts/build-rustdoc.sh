#!/usr/bin/env bash
# Builds the Rust API reference: rustdoc for the `undra` facade crate and the crates it re-exports
# (runtime, signals, query, ports, wire, meta), into target/doc/. Not committed: CI builds it and
# stage.sh publishes it at /reference/rust/.
#
#   bash site/scripts/build-rustdoc.sh
#
# `-D warnings` makes a missing doc, a broken intra-doc link or any other rustdoc warning fail the
# build (constitution R4: docs on every pub item). The theme is rustdoc's `ayu`, the closest of its
# three to the site, with the site's palette, font and icon laid over it (rustdoc-header.inc).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
[ -f scripts/env.sh ] && source scripts/env.sh >/dev/null 2>&1 || true

rm -rf "${CARGO_TARGET_DIR:-target}/doc" # nothing left over from another build (a cache) reaches the site
export RUSTDOCFLAGS="-D warnings --default-theme=ayu --html-in-header $ROOT/site/scripts/rustdoc-header.inc"
cargo doc --no-deps --locked \
  -p undra -p undra-runtime -p undra-signals -p undra-query -p undra-ports -p undra-wire -p undra-meta
test -f "${CARGO_TARGET_DIR:-target}/doc/undra/index.html"
