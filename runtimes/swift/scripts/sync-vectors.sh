#!/usr/bin/env bash
# Copies the shared wire contract vectors into the Swift test resources.
#
#   contract-tests/wire-vectors.json  ->  runtimes/swift/UndraRuntime/Tests/UndraRuntimeTests/Resources/wire-vectors.json
#
# SwiftPM packages cannot reference files outside their own directory, so the tests read a copy.
# Run this whenever contract-tests/wire-vectors.json changes and commit the result.
#
# Usage:
#   scripts/sync-vectors.sh           copy the vectors (creates the Resources directory if needed)
#   scripts/sync-vectors.sh --check   exit 1 if the copy is missing or differs (for CI)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
source_file="$repo_root/contract-tests/wire-vectors.json"
swift_root="$(cd "$here/.." && pwd)"
target_dir="$swift_root/UndraRuntime/Tests/UndraRuntimeTests/Resources"
target_file="$target_dir/wire-vectors.json"

if [[ ! -f "$source_file" ]]; then
  echo "sync-vectors: source not found: $source_file" >&2
  exit 2
fi

case "${1:-}" in
  "")
    mkdir -p "$target_dir"
    cp "$source_file" "$target_file"
    echo "sync-vectors: copied $(basename "$source_file") -> ${target_file#"$repo_root"/}"
    ;;
  --check)
    if [[ ! -f "$target_file" ]] || ! cmp -s "$source_file" "$target_file"; then
      echo "sync-vectors: ${target_file#"$repo_root"/} is missing or out of date; run runtimes/swift/scripts/sync-vectors.sh" >&2
      exit 1
    fi
    echo "sync-vectors: up to date"
    ;;
  *)
    echo "usage: sync-vectors.sh [--check]" >&2
    exit 2
    ;;
esac
