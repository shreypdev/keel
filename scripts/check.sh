#!/usr/bin/env bash
# Full local quality gate for the Rust workspace.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps 2>&1 | grep -E "warning|error" && { echo "doc warnings"; exit 1; } || true
echo "OK"
